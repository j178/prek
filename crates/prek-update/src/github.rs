use anyhow::{Context, Result, bail};
use prek_consts::env_vars::{EnvVars, EnvVarsRead};
use reqwest::{Client, Url};
use semver::Version;
use serde::Deserialize;

use crate::UpdateRequest;
use crate::receipt::ReleaseSource;

#[derive(Deserialize)]
struct GitHubRelease {
    tag_name: String,
    assets: Vec<Asset>,
    prerelease: bool,
}

pub(crate) struct Release {
    pub(crate) tag_name: String,
    pub(crate) version: Version,
    pub(crate) assets: Vec<Asset>,
}

#[derive(Deserialize)]
pub(crate) struct Asset {
    pub(crate) name: String,
    pub(crate) browser_download_url: String,
}

fn github_api(app_name: &str) -> Result<String> {
    let app_name = app_name.to_ascii_uppercase().replace('-', "_");
    let enterprise_variable = format!("{app_name}_INSTALLER_GHE_BASE_URL");
    let github_variable = format!("{app_name}_INSTALLER_GITHUB_BASE_URL");
    match (
        EnvVars.var(&enterprise_variable),
        EnvVars.var(&github_variable),
    ) {
        (Ok(_), Ok(_)) => {
            bail!("Only one of {enterprise_variable} and {github_variable} may be set");
        }
        (Ok(base), Err(_)) => Ok(Url::parse(&base)?.join("api/v3")?.to_string()),
        (Err(_), Ok(base)) => {
            let url = Url::parse(&base)?;
            let domain = url.domain().with_context(|| {
                format!("{github_variable} must contain a domain; use {enterprise_variable} for GitHub Enterprise")
            })?;
            let port = url
                .port()
                .map(|port| format!(":{port}"))
                .unwrap_or_default();
            Ok(format!("{}://api.{domain}{port}", url.scheme()))
        }
        (Err(_), Err(_)) => Ok("https://api.github.com".to_owned()),
    }
}

async fn get(client: &Client, url: &str, token: Option<&str>) -> Result<reqwest::Response> {
    let mut request = client
        .get(url)
        .header(reqwest::header::ACCEPT, "application/json")
        .header("X-GitHub-Api-Version", "2022-11-28");
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    Ok(request.send().await?.error_for_status()?)
}

pub(crate) async fn fetch_release(
    client: &Client,
    source: &ReleaseSource,
    request: UpdateRequest<'_>,
    token: Option<&str>,
) -> Result<Release> {
    if source.release_type != "github" {
        bail!("Unsupported release source: {}", source.release_type);
    }
    let api = github_api(&source.app_name)?;
    let releases_url = format!("{api}/repos/{}/{}/releases", source.owner, source.name);
    match request {
        UpdateRequest::SpecificTag(tag) => {
            let release = get(client, &format!("{releases_url}/tags/{tag}"), token)
                .await?
                .json::<GitHubRelease>()
                .await?;
            release.into_release(&source.app_name)
        }
        UpdateRequest::Latest => {
            // A release marked latest may lack an installer. In that case, or if the
            // endpoint is unavailable, select the newest installable stable version.
            if let Ok(response) = get(client, &format!("{releases_url}/latest"), token).await {
                if let Ok(release) = response.json::<GitHubRelease>().await {
                    if release.has_installer(&source.app_name) {
                        if let Ok(release) = release.into_release(&source.app_name) {
                            return Ok(release);
                        }
                    }
                }
            }

            let mut next = Some(releases_url);
            let mut latest: Option<Release> = None;
            while let Some(url) = next.take() {
                let response = get(client, &url, token).await?;
                next = response
                    .headers()
                    .get(reqwest::header::LINK)
                    .map(|header| header.to_str())
                    .transpose()?
                    .and_then(next_page)
                    .map(str::to_owned);
                for release in response.json::<Vec<GitHubRelease>>().await? {
                    if release.prerelease || !release.has_installer(&source.app_name) {
                        continue;
                    }
                    let Ok(release) = release.into_release(&source.app_name) else {
                        continue;
                    };
                    if latest
                        .as_ref()
                        .is_none_or(|latest| release.version >= latest.version)
                    {
                        latest = Some(release);
                    }
                }
            }
            latest.context("There are no stable releases available for prek")
        }
    }
}

fn next_page(header: &str) -> Option<&str> {
    header.split(',').find_map(|link| {
        let (url, parameters) = link.split_once(';')?;
        if parameters
            .split(';')
            .any(|parameter| parameter.trim() == "rel=\"next\"")
        {
            url.trim().strip_prefix('<')?.strip_suffix('>')
        } else {
            None
        }
    })
}

impl GitHubRelease {
    fn has_installer(&self, app_name: &str) -> bool {
        let prefix = format!("{app_name}-installer");
        self.assets
            .iter()
            .any(|asset| asset.name.starts_with(&prefix))
    }

    fn into_release(self, app_name: &str) -> Result<Release> {
        // cargo-dist supports plain versions, v-prefixed tags, and package-qualified tags.
        let tag = self.tag_name.rsplit('/').next().unwrap_or(&self.tag_name);
        let tag = tag
            .strip_prefix(app_name)
            .and_then(|tag| tag.strip_prefix('-'))
            .unwrap_or(tag);
        let version = tag.strip_prefix('v').unwrap_or(tag).parse::<Version>()?;
        Ok(Release {
            tag_name: self.tag_name,
            version,
            assets: self.assets,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Preserve the installer's GitHub/GHE endpoint rules, including invalid overrides.
    #[test]
    fn api_configuration() -> Result<()> {
        if crate::tests::is_child() {
            let expected = EnvVars.var("PREK_UPDATE_TEST_EXPECTED")?;
            let actual = github_api("some-app");
            if expected == "error" {
                assert!(actual.is_err());
            } else {
                assert_eq!(actual?, expected);
            }
            return Ok(());
        }

        for (github, enterprise, expected) in [
            (None, None, "https://api.github.com"),
            (
                Some("https://github.example"),
                None,
                "https://api.github.example",
            ),
            (
                Some("https://github.example:8443/ignored"),
                None,
                "https://api.github.example:8443",
            ),
            (Some("https://127.0.0.1"), None, "error"),
            (Some("not a URL"), None, "error"),
            (
                None,
                Some("https://github.example"),
                "https://github.example/api/v3",
            ),
            (
                None,
                Some("http://127.0.0.1:8080"),
                "http://127.0.0.1:8080/api/v3",
            ),
            (
                None,
                Some("https://github.example/prefix/"),
                "https://github.example/prefix/api/v3",
            ),
            (None, Some("not a URL"), "error"),
            (
                Some("https://github.example"),
                Some("https://enterprise.example"),
                "error",
            ),
        ] {
            let mut command = crate::tests::child()?;
            command
                .env_remove("SOME_APP_INSTALLER_GITHUB_BASE_URL")
                .env_remove("SOME_APP_INSTALLER_GHE_BASE_URL")
                .env("PREK_UPDATE_TEST_EXPECTED", expected);
            if let Some(github) = github {
                command.env("SOME_APP_INSTALLER_GITHUB_BASE_URL", github);
            }
            if let Some(enterprise) = enterprise {
                command.env("SOME_APP_INSTALLER_GHE_BASE_URL", enterprise);
            }
            crate::tests::check_child(&mut command)?;
        }
        Ok(())
    }

    /// Follow only the next page when GitHub includes several relations or extra parameters.
    #[test]
    fn pagination_links() {
        for (header, expected) in [
            (
                r#"<https://api.github.com/releases?page=1>; rel="prev", <https://api.github.com/releases?page=3>; rel="next", <https://api.github.com/releases?page=9>; rel="last"#,
                Some("https://api.github.com/releases?page=3"),
            ),
            (
                r#" <https://api.github.com/releases?page=2>; type="application/json"; rel="next" "#,
                Some("https://api.github.com/releases?page=2"),
            ),
            (
                r#"<https://api.github.com/releases?page=1>; rel="prev""#,
                None,
            ),
            ("", None),
        ] {
            assert_eq!(next_page(header), expected, "{header}");
        }
    }

    /// Accept cargo-dist tag formats while retaining the original tag for release links.
    #[test]
    fn release_tags() -> Result<()> {
        for (tag, version) in [
            ("1.2.3", "1.2.3"),
            ("v1.2.3", "1.2.3"),
            ("prek-1.2.3", "1.2.3"),
            ("prek-v1.2.3", "1.2.3"),
            ("releases/prek-v1.2.3-rc.1+build.2", "1.2.3-rc.1+build.2"),
        ] {
            let release = GitHubRelease {
                tag_name: tag.into(),
                assets: Vec::new(),
                prerelease: false,
            }
            .into_release("prek")?;
            assert_eq!(release.version, version.parse::<Version>()?);
            assert_eq!(release.tag_name, tag);
        }
        for tag in ["latest", "v1.2", "another-v1.2.3"] {
            let release = GitHubRelease {
                tag_name: tag.into(),
                assets: Vec::new(),
                prerelease: false,
            };
            assert!(release.into_release("prek").is_err(), "{tag}");
        }
        Ok(())
    }
}
