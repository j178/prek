use assert_fs::fixture::{FileWriteStr, PathChild, PathCreateDir};

use crate::common::{TestContext, cmd_snapshot, git_cmd};

/// Test basic Ruby hook with system Ruby
#[test]
fn system_ruby() {
    let context = TestContext::new();
    context.init_project();

    // Discover the actual system Ruby path
    let ruby_path = which::which("ruby")
        .expect("Ruby not found in PATH")
        .to_string_lossy()
        .to_string();

    context.write_pre_commit_config(&format!(
        indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: ruby-version
                name: ruby-version
                language: ruby
                entry: ruby --version
                language_version: system
                pass_filenames: false
                always_run: true
              - id: ruby-version-unspecified
                name: ruby-version-unspecified
                language: ruby
                entry: ruby --version
                pass_filenames: false
                always_run: true
              - id: ruby-version-path
                name: ruby-version-path
                language: ruby
                language_version: {}
                entry: ruby --version
                pass_filenames: false
                always_run: true
    "},
        ruby_path
    ));
    context.git_add(".");

    let filters = [(
        r"ruby (\d+\.\d+)\.\d+(?:p\d+)? \(\d{4}-\d{2}-\d{2} revision [0-9a-f]{0,10}\).*?\[.+\]",
        "ruby $1.X ([DATE] revision [HASH]) [FLAGS] [PLATFORM]",
    )]
    .into_iter()
    .chain(context.filters())
    .collect::<Vec<_>>();

    cmd_snapshot!(filters, context.run().arg("-v"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    ruby-version.............................................................Passed
    - hook id: ruby-version
    - duration: [TIME]

      ruby 3.4.X ([DATE] revision [HASH]) [FLAGS] [PLATFORM]
    ruby-version-unspecified.................................................Passed
    - hook id: ruby-version-unspecified
    - duration: [TIME]

      ruby 3.4.X ([DATE] revision [HASH]) [FLAGS] [PLATFORM]
    ruby-version-path........................................................Passed
    - hook id: ruby-version-path
    - duration: [TIME]

      ruby 3.4.X ([DATE] revision [HASH]) [FLAGS] [PLATFORM]

    ----- stderr -----
    ");
}

/// Test that `language_version: default` works
#[test]
fn language_version_default() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: ruby-default
                name: ruby-default
                language: ruby
                entry: ruby --version
                language_version: default
                pass_filenames: false
                always_run: true
    "});
    context.git_add(".");

    let filters = [(
        r"ruby (\d+\.\d+)\.\d+(?:p\d+)? \(\d{4}-\d{2}-\d{2} revision [0-9a-f]{0,10}\).*?\[.+\]",
        "ruby $1.X ([DATE] revision [HASH]) [FLAGS] [PLATFORM]",
    )]
    .into_iter()
    .chain(context.filters())
    .collect::<Vec<_>>();

    cmd_snapshot!(filters, context.run().arg("-v"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    ruby-default.............................................................Passed
    - hook id: ruby-default
    - duration: [TIME]

      ruby 3.4.X ([DATE] revision [HASH]) [FLAGS] [PLATFORM]

    ----- stderr -----
    ");
}

/// Test basic Ruby hook with a specified (and available) version of Ruby
#[test]
fn specific_ruby_available() {
    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: ruby-version-prefixed
                name: ruby-version-prefixed
                language: ruby
                entry: ruby --version
                language_version: ruby3.4
                pass_filenames: false
                always_run: true
              - id: ruby-version
                name: ruby-version
                language: ruby
                entry: ruby --version
                language_version: '3.4'
                pass_filenames: false
                always_run: true
              - id: ruby-version-range-min
                name: ruby-version-range-min
                language: ruby
                entry: ruby --version
                language_version: '>=3.2'
                pass_filenames: false
                always_run: true
              - id: ruby-version-range-max
                name: ruby-version-range-max
                language: ruby
                entry: ruby --version
                language_version: '<4.0'
                pass_filenames: false
                always_run: true
              - id: ruby-version-constrained-range
                name: ruby-version-constrained-range
                language: ruby
                entry: ruby --version
                language_version: '>=3.2, <4'
                pass_filenames: false
                always_run: true
    "});
    context.git_add(".");

    let filters = [(
        r"ruby (\d+\.\d+)\.\d+(?:p\d+)? \(\d{4}-\d{2}-\d{2} revision [0-9a-f]{0,10}\).*?\[.+\]",
        "ruby $1.X ([DATE] revision [HASH]) [FLAGS] [PLATFORM]",
    )]
    .into_iter()
    .chain(context.filters())
    .collect::<Vec<_>>();

    cmd_snapshot!(filters, context.run().arg("-v"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    ruby-version-prefixed....................................................Passed
    - hook id: ruby-version-prefixed
    - duration: [TIME]

      ruby 3.4.X ([DATE] revision [HASH]) [FLAGS] [PLATFORM]
    ruby-version.............................................................Passed
    - hook id: ruby-version
    - duration: [TIME]

      ruby 3.4.X ([DATE] revision [HASH]) [FLAGS] [PLATFORM]
    ruby-version-range-min...................................................Passed
    - hook id: ruby-version-range-min
    - duration: [TIME]

      ruby 3.4.X ([DATE] revision [HASH]) [FLAGS] [PLATFORM]
    ruby-version-range-max...................................................Passed
    - hook id: ruby-version-range-max
    - duration: [TIME]

      ruby 3.4.X ([DATE] revision [HASH]) [FLAGS] [PLATFORM]
    ruby-version-constrained-range...........................................Passed
    - hook id: ruby-version-constrained-range
    - duration: [TIME]

      ruby 3.4.X ([DATE] revision [HASH]) [FLAGS] [PLATFORM]

    ----- stderr -----
    ");
}

/// Test basic Ruby hook with a specified (and unavailable) version of Ruby
#[test]
fn specific_ruby_unavailable() {
    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: ruby-version
                name: ruby-version
                language: ruby
                entry: ruby --version
                language_version: 3.1.3
                pass_filenames: false
                always_run: true
    "});
    context.git_add(".");

    let filters = [(
        r"ruby (\d+\.\d+)\.\d+(?:p\d+)? \(\d{4}-\d{2}-\d{2} revision [0-9a-f]{0,10}\).*?\[.+\]",
        "ruby $1.X ([DATE] revision [HASH]) [FLAGS] [PLATFORM]",
    )]
    .into_iter()
    .chain(context.filters())
    .collect::<Vec<_>>();

    #[cfg(target_os = "windows")]
    cmd_snapshot!(filters, context.run().arg("-v"), @r"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to install hook `ruby-version`
      caused by: Failed to install Ruby
      caused by: No suitable Ruby found for request: 3.1.3

    Ruby language only supports system Ruby on Windows.
    Please install Ruby from https://rubyinstaller.org/
    ");

    #[cfg(not(target_os = "windows"))]
    cmd_snapshot!(filters, context.run().arg("-v"), @r"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to install hook `ruby-version`
      caused by: Failed to install Ruby
      caused by: No suitable Ruby found for request: 3.1.3

    Detected version manager(s): brew

    You can install the required Ruby version using:
      brew install ruby  # Installs latest version
      # Note: Homebrew typically installs the latest Ruby version.
      # For specific versions, consider using a version manager like rbenv or mise.
    ");
}

/// Test Ruby hook with `additional_dependencies` and `require` statement
#[test]
fn additional_gem_dependencies() -> anyhow::Result<()> {
    let context = TestContext::new();
    context.init_project();

    // Create a Ruby script that uses a gem from additional_dependencies
    // Use 'rspec' - a gem that's NOT bundled with Ruby
    context
        .work_dir()
        .child("test_script.rb")
        .write_str(indoc::indoc! {r"
            require 'rspec'
            puts RSpec::Version::STRING
        "})?;

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: test-gem-require
                name: test-gem-require
                language: ruby
                entry: ruby test_script.rb
                language_version: system
                additional_dependencies: ["rspec"]
                pass_filenames: false
                always_run: true
              - id: test-gem-require-versioned
                name: test-gem-require-versioned
                language: ruby
                entry: ruby test_script.rb
                language_version: system
                additional_dependencies: ["rspec:3.12.0"]
                pass_filenames: false
                always_run: true
              - id: test-gem-require-missing
                name: test-gem-require-missing
                language: ruby
                entry: ruby test_script.rb
                language_version: system
                pass_filenames: false
                always_run: true
    "#});
    context.git_add(".");

    let filters = [
        // Normalize unpinned rspec version (only for test-gem-require, not test-gem-require-versioned)
        (
            r"(- hook id: test-gem-require\n- duration: .*?\n\n)  \d+\.\d+\.\d+",
            "$1  X.Y.Z",
        ),
        // Normalize Ruby internal paths
        (r"<internal:[^>]+>:\d+:in", "<internal:[RUBY_LIB]>:[X]:in"),
    ]
    .into_iter()
    .chain(context.filters())
    .collect::<Vec<_>>();

    cmd_snapshot!(filters, context.run().arg("-v"), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    test-gem-require.........................................................Passed
    - hook id: test-gem-require
    - duration: [TIME]

      X.Y.Z
    test-gem-require-versioned...............................................Passed
    - hook id: test-gem-require-versioned
    - duration: [TIME]

      3.12.0
    test-gem-require-missing.................................................Failed
    - hook id: test-gem-require-missing
    - duration: [TIME]
    - exit code: 1

      <internal:[RUBY_LIB]>:[X]:in 'Kernel#require': cannot load such file -- rspec (LoadError)
      	from <internal:[RUBY_LIB]>:[X]:in 'Kernel#require'
      	from test_script.rb:1:in '<main>'

    ----- stderr -----
    ");

    Ok(())
}

/// Test Ruby hook with gemspec
#[test]
fn gemspec_workflow() -> anyhow::Result<()> {
    let context = TestContext::new();
    context.init_project();

    // Create a simple gemspec
    context
        .work_dir()
        .child("test_gem.gemspec")
        .write_str(indoc::indoc! {r#"
            Gem::Specification.new do |spec|
              spec.name          = "test_gem"
              spec.version       = "0.1.0"
              spec.authors       = ["Test"]
              spec.email         = ["test@example.com"]
              spec.summary       = "Test gem"
              spec.files         = ["lib/test_gem.rb"]
              spec.require_paths = ["lib"]
            end
        "#})?;

    // Create lib directory and file
    context.work_dir().child("lib").create_dir_all()?;
    context
        .work_dir()
        .child("lib/test_gem.rb")
        .write_str(indoc::indoc! {r#"
            module TestGem
              def self.hello
                "Hello from TestGem"
              end
            end
        "#})?;

    // Create test script
    context
        .work_dir()
        .child("test_script.rb")
        .write_str(indoc::indoc! {r"
            require 'test_gem'
            puts TestGem.hello
        "})?;

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: test-gemspec
                name: test-gemspec
                language: ruby
                entry: ruby -I lib test_script.rb
                language_version: system
                pass_filenames: false
                always_run: true
    "});
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run().arg("-v"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    test-gemspec.............................................................Passed
    - hook id: test-gemspec
    - duration: [TIME]

      Hello from TestGem

    ----- stderr -----
    ");

    Ok(())
}

/// Test environment isolation between Ruby hooks
#[test]
fn environment_isolation() -> anyhow::Result<()> {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: hook1
                name: hook1
                language: ruby
                entry: ruby -e "puts 'hook1=' + ENV['GEM_HOME']"
                language_version: system
                pass_filenames: false
                always_run: true
                verbose: true
              - id: hook2
                name: hook2
                language: ruby
                entry: ruby -e "puts 'hook2=' + ENV['GEM_HOME']"
                language_version: system
                pass_filenames: false
                always_run: true
                verbose: true
              - id: hook3
                name: hook3
                language: ruby
                entry: ruby -e "puts 'hook3=' + ENV['GEM_HOME']"
                language_version: system
                additional_dependencies: ["rspec"]
                pass_filenames: false
                always_run: true
                verbose: true
              - id: hook4
                name: hook4
                language: ruby
                entry: ruby -e "puts 'hook4=' + ENV['GEM_HOME']"
                language_version: system
                additional_dependencies: ["webrick"]
                pass_filenames: false
                always_run: true
                verbose: true
              - id: hook5
                name: hook5
                language: ruby
                entry: ruby -e "puts 'hook5=' + ENV['GEM_HOME']"
                language_version: system
                additional_dependencies: ["rspec"]
                pass_filenames: false
                always_run: true
                verbose: true
    "#});
    context.git_add(".");

    let output = context.run().output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "Command failed\nEXIT CODE: {:?}\nSTDOUT:\n{}\nSTDERR:\n{}",
        output.status.code(),
        stdout,
        stderr
    );

    // Extract GEM_HOME paths from each hook's output
    let extract_gem_home = |hook_id: &str| -> String {
        let prefix = format!("{hook_id}=");
        stdout
            .lines()
            .find_map(|line| line.trim().strip_prefix(&prefix))
            .unwrap_or_else(|| panic!("Failed to extract GEM_HOME for {hook_id}"))
            .to_string()
    };

    let hook1_gem_home = extract_gem_home("hook1");
    let hook2_gem_home = extract_gem_home("hook2");
    let hook3_gem_home = extract_gem_home("hook3");
    let hook4_gem_home = extract_gem_home("hook4");
    let hook5_gem_home = extract_gem_home("hook5");

    // Verify isolation: hook1 == hook2 (same dependencies (none))
    assert_eq!(
        hook1_gem_home, hook2_gem_home,
        "hook1 and hook2 should share the same environment (both have no additional_dependencies)"
    );

    // Verify isolation: hook3 == hook5 (same dependencies (rspec))
    assert_eq!(
        hook3_gem_home, hook5_gem_home,
        "hook3 and hook5 should share the same environment (both have the same additional_dependencies)"
    );

    // Verify isolation: hook1 != hook3 (different dependencies)
    assert_ne!(
        hook1_gem_home, hook3_gem_home,
        "hook1 and hook3 should have different environments (hook3 has rspec)"
    );

    // Verify isolation: hook1 != hook4 (different dependencies)
    assert_ne!(
        hook1_gem_home, hook4_gem_home,
        "hook1 and hook4 should have different environments (hook4 has webrick)"
    );

    // Verify isolation: hook3 != hook4 (different dependencies)
    assert_ne!(
        hook3_gem_home, hook4_gem_home,
        "hook3 and hook4 should have different environments (different gems)"
    );

    // Run the command again to check that the environments are reused
    let output = context.run().output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "Command failed\nEXIT CODE: {:?}\nSTDOUT:\n{}\nSTDERR:\n{}",
        output.status.code(),
        stdout,
        stderr
    );

    let hook1_gem_home_v2 = extract_gem_home("hook1");
    let hook2_gem_home_v2 = extract_gem_home("hook2");
    let hook3_gem_home_v2 = extract_gem_home("hook3");
    let hook4_gem_home_v2 = extract_gem_home("hook4");
    let hook5_gem_home_v2 = extract_gem_home("hook5");

    assert_eq!(
        hook1_gem_home, hook1_gem_home_v2,
        "hook1 should reuse the same environment on a second run"
    );

    assert_eq!(
        hook2_gem_home, hook2_gem_home_v2,
        "hook2 should reuse the same environment on a second run"
    );

    assert_eq!(
        hook3_gem_home, hook3_gem_home_v2,
        "hook3 should reuse the same environment on a second run"
    );

    assert_eq!(
        hook4_gem_home, hook4_gem_home_v2,
        "hook4 should reuse the same environment on a second run"
    );

    assert_eq!(
        hook5_gem_home, hook5_gem_home_v2,
        "hook5 should reuse the same environment on a second run"
    );

    Ok(())
}

/// Test local Ruby hook repository with gemspec build and install
#[test]
fn local_hook_with_gemspec() -> anyhow::Result<()> {
    let context = TestContext::new();
    context.init_project();

    // Create a local hook repository with a gemspec
    let hook_repo = context.work_dir().child("my-hook-repo");
    hook_repo.create_dir_all()?;

    // Create the gemspec
    hook_repo
        .child("my_hook.gemspec")
        .write_str(indoc::indoc! {r#"
            Gem::Specification.new do |spec|
              spec.name          = "my_hook"
              spec.version       = "0.1.0"
              spec.authors       = ["Test"]
              spec.email         = ["test@example.com"]
              spec.summary       = "Test hook gem"
              spec.files         = ["bin/my-hook"]
              spec.executables   = ["my-hook"]
              spec.bindir        = "bin"
            end
        "#})?;

    // Create executable
    hook_repo.child("bin").create_dir_all()?;
    hook_repo.child("bin/my-hook").write_str(indoc::indoc! {r#"
        #!/usr/bin/env ruby
        puts "Hook executed from gem!"
    "#})?;

    // Create .pre-commit-hooks.yaml manifest
    hook_repo
        .child(".pre-commit-hooks.yaml")
        .write_str(indoc::indoc! {r"
            - id: my-hook
              name: My Hook
              entry: my-hook
              language: ruby
              pass_filenames: false
        "})?;

    // Initialize git repo in the hook directory (separate from main project)
    let output = git_cmd(&hook_repo).args(["init"]).output()?;
    assert!(output.status.success(), "git init failed: {output:?}");

    // Configure git user for this repo
    git_cmd(&hook_repo)
        .args(["config", "user.name", "Test User"])
        .output()?;

    git_cmd(&hook_repo)
        .args(["config", "user.email", "test@example.com"])
        .output()?;

    let output = git_cmd(&hook_repo).args(["add", "."]).output()?;
    assert!(output.status.success(), "git add failed: {output:?}");

    let output = git_cmd(&hook_repo)
        .args(["commit", "-m", "Initial commit"])
        .output()?;
    assert!(output.status.success(), "git commit failed: {output:?}");

    // Get the commit SHA
    let rev_output = git_cmd(&hook_repo).args(["rev-parse", "HEAD"]).output()?;
    assert!(rev_output.status.success(), "git rev-parse failed");
    let rev = String::from_utf8_lossy(&rev_output.stdout)
        .trim()
        .to_string();

    // Configure prek to use this local repo
    context.write_pre_commit_config(&indoc::formatdoc! {r"
            repos:
              - repo: {}
                rev: {}
                hooks:
                  - id: my-hook
                    name: my-hook
                    entry: my-hook
                    language: ruby
                    pass_filenames: false
                    always_run: true
        ",
        hook_repo.to_path_buf().display(),
        rev
    });
    context.git_add(".pre-commit-config.yaml");

    cmd_snapshot!(context.filters(), context.run().arg("-v"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    my-hook..................................................................Passed
    - hook id: my-hook
    - duration: [TIME]

      Hook executed from gem!

    ----- stderr -----
    ");

    Ok(())
}

/// Test Ruby hook with native gem (C extension)
#[test]
fn native_gem_dependency() -> anyhow::Result<()> {
    let context = TestContext::new();
    context.init_project();

    // Create a Ruby script that uses msgpack (small native gem that compiles quickly)
    context
        .work_dir()
        .child("check_msgpack.rb")
        .write_str(indoc::indoc! {r#"
            #!/usr/bin/env ruby
            require 'msgpack'

            # Test that the native extension works
            data = { "hello" => "world", "number" => 42 }
            packed = MessagePack.pack(data)
            unpacked = MessagePack.unpack(packed)

            puts "MessagePack native extension working!"
            puts "Packed size: #{packed.bytesize} bytes"
        "#})?;

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: test-native-gem
                name: test-native-gem
                language: ruby
                entry: ruby check_msgpack.rb
                additional_dependencies: ['msgpack']
                pass_filenames: false
                always_run: true
    "});
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run().arg("-v"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    test-native-gem..........................................................Passed
    - hook id: test-native-gem
    - duration: [TIME]

      MessagePack native extension working!
      Packed size: 21 bytes

    ----- stderr -----
    ");

    Ok(())
}

/// Test that multiple gemspecs in a hook repo are built and installed together,
/// with all dependencies resolved in a single pass.
#[test]
fn multiple_gemspecs() -> anyhow::Result<()> {
    let context = TestContext::new();
    context.init_project();

    let hook_repo = context.work_dir().child("multi-gem-repo");
    hook_repo.create_dir_all()?;

    // First gemspec: depends on `rainbow`
    hook_repo
        .child("gem_one.gemspec")
        .write_str(indoc::indoc! {r#"
            Gem::Specification.new do |spec|
              spec.name          = "gem_one"
              spec.version       = "0.1.0"
              spec.authors       = ["Test"]
              spec.summary       = "First gem"
              spec.files         = ["bin/gem-one"]
              spec.executables   = ["gem-one"]
              spec.bindir        = "bin"
              spec.add_dependency "rainbow", "~> 3.0"
            end
        "#})?;

    // Second gemspec: also depends on `rainbow` (overlapping) plus `unicode-display_width`.
    // This tests that --explain de-duplicates shared dependencies across gemspecs.
    hook_repo.child("lib").create_dir_all()?;
    hook_repo
        .child("lib/gem_two.rb")
        .write_str("module GemTwo; end\n")?;
    hook_repo
        .child("gem_two.gemspec")
        .write_str(indoc::indoc! {r#"
            Gem::Specification.new do |spec|
              spec.name          = "gem_two"
              spec.version       = "0.1.0"
              spec.authors       = ["Test"]
              spec.summary       = "Second gem"
              spec.files         = ["lib/gem_two.rb"]
              spec.add_dependency "rainbow", "~> 3.0"
              spec.add_dependency "unicode-display_width", "~> 3.0"
            end
        "#})?;

    // Executable that requires both gems' dependencies
    hook_repo.child("bin").create_dir_all()?;
    hook_repo.child("bin/gem-one").write_str(indoc::indoc! {r#"
        #!/usr/bin/env ruby
        require 'rainbow'
        require 'unicode/display_width'
        puts "rainbow=#{Gem.loaded_specs['rainbow'].version}"
        puts "udw=#{Gem.loaded_specs['unicode-display_width'].version}"
    "#})?;

    hook_repo
        .child(".pre-commit-hooks.yaml")
        .write_str(indoc::indoc! {r"
            - id: multi-gem
              name: Multi Gem
              entry: gem-one
              language: ruby
              pass_filenames: false
        "})?;

    let output = git_cmd(&hook_repo).args(["init"]).output()?;
    assert!(output.status.success(), "git init failed: {output:?}");
    git_cmd(&hook_repo)
        .args(["config", "user.name", "Test User"])
        .output()?;
    git_cmd(&hook_repo)
        .args(["config", "user.email", "test@example.com"])
        .output()?;
    let output = git_cmd(&hook_repo).args(["add", "."]).output()?;
    assert!(output.status.success(), "git add failed: {output:?}");
    let output = git_cmd(&hook_repo)
        .args(["commit", "-m", "Initial commit"])
        .output()?;
    assert!(output.status.success(), "git commit failed: {output:?}");

    let rev_output = git_cmd(&hook_repo).args(["rev-parse", "HEAD"]).output()?;
    let rev = String::from_utf8_lossy(&rev_output.stdout)
        .trim()
        .to_string();

    context.write_pre_commit_config(&indoc::formatdoc! {r"
            repos:
              - repo: {}
                rev: {}
                hooks:
                  - id: multi-gem
                    always_run: true
        ",
        hook_repo.to_path_buf().display(),
        rev
    });
    context.git_add(".pre-commit-config.yaml");

    let filters = [
        (r"rainbow=\d+\.\d+\.\d+", "rainbow=X.Y.Z"),
        (r"udw=\d+\.\d+\.\d+", "udw=X.Y.Z"),
    ]
    .into_iter()
    .chain(context.filters())
    .collect::<Vec<_>>();

    cmd_snapshot!(filters, context.run().arg("-v"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Multi Gem................................................................Passed
    - hook id: multi-gem
    - duration: [TIME]

      rainbow=X.Y.Z
      udw=X.Y.Z

    ----- stderr -----
    ");

    Ok(())
}

/// Test that pre-built platform-specific gems skip compilation while source gems
/// compile natively. Installs `sqlite3` (publishes precompiled platform gems) and
/// `msgpack` (source-only, requires C compilation) side by side.
#[test]
fn prebuilt_vs_compiled_gems() -> anyhow::Result<()> {
    let context = TestContext::new();
    context.init_project();

    context
        .work_dir()
        .child("check_gems.rb")
        .write_str(indoc::indoc! {r#"
            require 'sqlite3'
            require 'msgpack'

            db = SQLite3::Database.new(":memory:")
            db.execute("CREATE TABLE t (v TEXT)")
            db.execute("INSERT INTO t VALUES (?)", [MessagePack.pack("ok")])
            puts "sqlite3=#{SQLite3::VERSION} msgpack=#{MessagePack::VERSION}"
        "#})?;

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: test-native-gems
                name: test-native-gems
                language: ruby
                entry: ruby check_gems.rb
                additional_dependencies: ['sqlite3', 'msgpack']
                pass_filenames: false
                always_run: true
    "});
    context.git_add(".");

    let filters = [
        (r"sqlite3=\d+\.\d+\.\d+", "sqlite3=X.Y.Z"),
        (r"msgpack=\d+\.\d+\.\d+", "msgpack=X.Y.Z"),
    ]
    .into_iter()
    .chain(context.filters())
    .collect::<Vec<_>>();

    cmd_snapshot!(filters, context.run().arg("-v"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    test-native-gems.........................................................Passed
    - hook id: test-native-gems
    - duration: [TIME]

      sqlite3=X.Y.Z msgpack=X.Y.Z

    ----- stderr -----
    ");

    // Verify installation methods by inspecting the gem directories.
    // Pre-built gems include a platform suffix (e.g. sqlite3-X.Y.Z-x86_64-linux-gnu),
    // while compiled-from-source gems do not (e.g. msgpack-X.Y.Z).
    let hooks_dir = context.home_dir().join("hooks");
    let gems_dir = fs_err::read_dir(&hooks_dir)?
        .filter_map(Result::ok)
        .find(|e| e.file_name().to_string_lossy().starts_with("ruby-"))
        .map(|e| e.path().join("gems").join("gems"))
        .expect("No ruby hook directory found");

    let gem_dirs: Vec<String> = fs_err::read_dir(&gems_dir)?
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();

    // sqlite3 should have a platform suffix (precompiled)
    let sqlite3_dir = gem_dirs
        .iter()
        .find(|d| d.starts_with("sqlite3-"))
        .expect("sqlite3 gem not found");
    assert!(
        sqlite3_dir.contains("x86_64-linux")
            || sqlite3_dir.contains("aarch64-linux")
            || sqlite3_dir.contains("arm64-darwin")
            || sqlite3_dir.contains("x64-mingw"),
        "sqlite3 should be a prebuilt platform gem, got: {sqlite3_dir}"
    );

    // msgpack should NOT have a platform suffix (compiled from source)
    let msgpack_dir = gem_dirs
        .iter()
        .find(|d| d.starts_with("msgpack-"))
        .expect("msgpack gem not found");
    assert!(
        !msgpack_dir.contains("linux")
            && !msgpack_dir.contains("darwin")
            && !msgpack_dir.contains("mingw"),
        "msgpack should be compiled from source (no platform suffix), got: {msgpack_dir}"
    );

    Ok(())
}

/// Test Ruby hook that processes files
#[test]
fn process_files() -> anyhow::Result<()> {
    let context = TestContext::new();
    context.init_project();

    // Create a Ruby script that validates file extensions
    context
        .work_dir()
        .child("check_ruby.rb")
        .write_str(indoc::indoc! {r#"
            ARGV.sort.each do |file|
              unless file.end_with?('.rb')
                puts "Error: #{file} is not a Ruby file"
                exit 1
              end
              puts "OK: #{file}"
            end
        "#})?;

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: check-ruby-files
                name: check-ruby-files
                language: ruby
                entry: ruby check_ruby.rb
                language_version: system
                files: \.rb$
                verbose: true
    "});

    // Create a Ruby file
    context
        .work_dir()
        .child("test.rb")
        .write_str("puts 'hello'")?;
    // Create a text file
    context.work_dir().child("test.txt").write_str("hello")?;

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    check-ruby-files.........................................................Passed
    - hook id: check-ruby-files
    - duration: [TIME]

      OK: check_ruby.rb
      OK: test.rb

    ----- stderr -----
    ");

    Ok(())
}
