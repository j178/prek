use std::path::Path;
use std::sync::LazyLock;

use anyhow::Result;
use regex::Regex;

use crate::hook::Hook;
use crate::hooks::HookOutput;
use crate::hooks::pre_commit_hooks::{FilenamesArgs, hook_filenames, parse_hook_args};
use crate::hooks::run_concurrent_file_checks;
use crate::run::INTERNAL_CONCURRENCY;

/// Some Git configs append a detached signature to the commit message file; it is not
/// part of the authored message and never contains a real `Signed-off-by` trailer.
const PGP_SIGNATURE_MARKER: &str = "-----BEGIN PGP SIGNATURE-----";

/// Matches a single `Signed-off-by` trailer line, per Git's own trailer convention.
/// The trailer keyword is case-sensitive; the email is only checked for a single `@`
/// separating a non-empty local part and domain, matching the Developer Certificate of
/// Origin's own (lenient) expectations rather than full email-address validation.
static SIGNOFF_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^Signed-off-by: .+ <[^@]+@[^@]+>$").expect("sign-off pattern must be valid")
});

/// Runs the `check-dco-signoff` hook.
pub(crate) async fn run(hook: &Hook, filenames: &[&Path]) -> Result<HookOutput> {
    let args: FilenamesArgs = parse_hook_args(hook)?;
    run_concurrent_file_checks(
        hook_filenames(&args.filenames, filenames),
        *INTERNAL_CONCURRENCY,
        |filename| check_file(hook.project().relative_path(), filename),
    )
    .await
}

async fn check_file(file_base: &Path, filename: &Path) -> Result<HookOutput> {
    let file_path = file_base.join(filename);
    let message = fs_err::tokio::read_to_string(&file_path).await?;

    if has_valid_signoff(&message) {
        return Ok(HookOutput::unchanged(0, Vec::new()));
    }

    Ok(HookOutput::unchanged(1, missing_signoff_message(filename)))
}

/// Returns true if `message` contains at least one valid `Signed-off-by` trailer.
fn has_valid_signoff(message: &str) -> bool {
    strip_pgp_signature(message)
        .lines()
        .any(|line| SIGNOFF_RE.is_match(line))
}

/// Strips a trailing PGP signature block, if present.
fn strip_pgp_signature(message: &str) -> &str {
    match message.find(PGP_SIGNATURE_MARKER) {
        Some(index) => &message[..index],
        None => message,
    }
}

fn missing_signoff_message(filename: &Path) -> Vec<u8> {
    format!(
        "{}: no `Signed-off-by` trailer found\n\n{}",
        filename.display(),
        indoc::indoc! {"
            This commit must be signed off per the Developer Certificate of Origin
            (https://developercertificate.org/), certifying you wrote it or otherwise
            have the right to submit it.

            To sign off this commit:
              git commit -s

            To sign off commits already made on this branch:
              git rebase --exec 'git commit --amend --no-edit -s' <base-commit>
        "}
    )
    .into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::tempdir;

    async fn create_test_file(
        dir: &tempfile::TempDir,
        name: &str,
        content: &str,
    ) -> Result<PathBuf> {
        let file_path = dir.path().join(name);
        fs_err::tokio::write(&file_path, content).await?;
        Ok(file_path)
    }

    #[tokio::test]
    async fn valid_signoff_passes() -> Result<()> {
        let dir = tempdir()?;
        let content = "Fix the thing\n\nSigned-off-by: Jane Doe <jane@example.com>\n";
        let file_path = create_test_file(&dir, "MSG", content).await?;
        let result = check_file(Path::new(""), &file_path).await?;
        assert_eq!(result.exit_status, 0);
        assert!(result.output.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn missing_signoff_fails() -> Result<()> {
        let dir = tempdir()?;
        let content = "Fix the thing\n\nNo trailer here.\n";
        let file_path = create_test_file(&dir, "MSG", content).await?;
        let result = check_file(Path::new(""), &file_path).await?;
        assert_eq!(result.exit_status, 1);
        let output = String::from_utf8_lossy(&result.output);
        assert!(output.contains("no `Signed-off-by` trailer found"));
        assert!(output.contains("git commit -s"));
        assert!(output.contains("git rebase --exec"));
        Ok(())
    }

    #[tokio::test]
    async fn malformed_email_fails() -> Result<()> {
        let dir = tempdir()?;
        // No `@` in the angle-bracketed part is not a valid trailer.
        let content = "Fix the thing\n\nSigned-off-by: Jane Doe <not-an-email>\n";
        let file_path = create_test_file(&dir, "MSG", content).await?;
        let result = check_file(Path::new(""), &file_path).await?;
        assert_eq!(result.exit_status, 1);
        Ok(())
    }

    #[tokio::test]
    async fn multiple_trailers_pass_when_one_is_valid() -> Result<()> {
        let dir = tempdir()?;
        let content = "\
Fix the thing

Co-authored-by: Alex Roe <not-an-email>
Signed-off-by: Jane Doe <jane@example.com>
Signed-off-by: Alex Roe <not-an-email>
";
        let file_path = create_test_file(&dir, "MSG", content).await?;
        let result = check_file(Path::new(""), &file_path).await?;
        assert_eq!(result.exit_status, 0);
        Ok(())
    }

    #[tokio::test]
    async fn co_authored_by_alone_does_not_satisfy_signoff() -> Result<()> {
        let dir = tempdir()?;
        let content = "Fix the thing\n\nCo-authored-by: Alex Roe <alex@example.com>\n";
        let file_path = create_test_file(&dir, "MSG", content).await?;
        let result = check_file(Path::new(""), &file_path).await?;
        assert_eq!(result.exit_status, 1);
        Ok(())
    }

    #[tokio::test]
    async fn pgp_signature_suffixed_message_is_stripped_before_checking() -> Result<()> {
        let dir = tempdir()?;
        // The trailer is above the signature block, as Git would produce it; the
        // signature armor itself must not be scanned for (and never contains) a trailer.
        let content = "\
Fix the thing

Signed-off-by: Jane Doe <jane@example.com>
-----BEGIN PGP SIGNATURE-----

iQEzBAABCAAdFiEE...
-----END PGP SIGNATURE-----
";
        let file_path = create_test_file(&dir, "MSG", content).await?;
        let result = check_file(Path::new(""), &file_path).await?;
        assert_eq!(result.exit_status, 0);
        Ok(())
    }

    #[tokio::test]
    async fn signoff_inside_pgp_signature_block_does_not_count() -> Result<()> {
        let dir = tempdir()?;
        // A trailer-shaped line that only appears after the signature marker must be
        // ignored, since everything from the marker onward is stripped.
        let content = "\
Fix the thing
-----BEGIN PGP SIGNATURE-----

Signed-off-by: Jane Doe <jane@example.com>
-----END PGP SIGNATURE-----
";
        let file_path = create_test_file(&dir, "MSG", content).await?;
        let result = check_file(Path::new(""), &file_path).await?;
        assert_eq!(result.exit_status, 1);
        Ok(())
    }

    #[test]
    fn signoff_regex_rejects_lowercase_keyword() {
        assert!(!SIGNOFF_RE.is_match("signed-off-by: Jane Doe <jane@example.com>"));
    }

    #[test]
    fn signoff_regex_requires_single_at_on_each_side() {
        assert!(!SIGNOFF_RE.is_match("Signed-off-by: Jane Doe <jane@@example.com>"));
        assert!(!SIGNOFF_RE.is_match("Signed-off-by: Jane Doe <@example.com>"));
        assert!(!SIGNOFF_RE.is_match("Signed-off-by: Jane Doe <jane@>"));
    }
}
