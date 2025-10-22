use assert_fs::fixture::{FileWriteStr, PathChild};

use crate::common::{TestContext, cmd_snapshot};

#[test]
fn health_check() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: lua
                name: lua
                language: lua
                entry: lua -e 'print("Hello from Lua!")'
                always_run: true
                verbose: true
                pass_filenames: false
    "#});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    lua......................................................................Passed
    - hook id: lua
    - duration: [TIME]
      Hello from Lua!

    ----- stderr -----
    "#);

    // Run again to check `health_check` works correctly.
    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    lua......................................................................Passed
    - hook id: lua
    - duration: [TIME]
      Hello from Lua!

    ----- stderr -----
    "#);
}

/// Test rockspec file installation.
#[test]
fn rockspec_installation() -> anyhow::Result<()> {
    let context = TestContext::new();
    context.init_project();

    // Create a simple rockspec file
    context
        .work_dir()
        .child("test-1.0-1.rockspec")
        .write_str(indoc::indoc! {r#"
        package = "test"
        version = "1.0-1"
        source = {
            url = "git://github.com/example/test.git",
            tag = "v1.0"
        }
        dependencies = {
            "lua >= 5.1",
            "luafilesystem >= 1.8.0"
        }
        build = {
            type = "builtin",
            modules = {
                test = "test.lua"
            }
        }
    "#})?;

    // Create a simple Lua module
    context
        .work_dir()
        .child("test.lua")
        .write_str(indoc::indoc! {r#"
        local lfs = require("lfs")
        local test = {}
        function test.hello()
            return "Hello from test module!"
        end
        return test
    "#})?;

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: lua
                name: lua
                language: lua
                entry: lua -e 'local test = require("test"); print(test.hello())'
                always_run: true
                verbose: true
                pass_filenames: false
    "#});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    lua......................................................................Passed
    - hook id: lua
    - duration: [TIME]
      Hello from test module!

    ----- stderr -----
    "#);

    Ok(())
}

/// Test invalid version handling.
#[test]
fn invalid_version() {
    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: local
                name: local
                language: lua
                entry: lua -v
                language_version: 'invalid-version' # invalid version
                always_run: true
                verbose: true
                pass_filenames: false
    "});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Hook `local` is invalid
      caused by: Invalid `language_version` value: `invalid-version`
    "#);
}

/// Test that stderr from hooks is captured and shown to the user.
#[test]
fn hook_stderr() -> anyhow::Result<()> {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: local
                name: local
                language: lua
                entry: lua ./hook.lua
    "});

    context
        .work_dir()
        .child("hook.lua")
        .write_str("io.stderr:write('How are you\\n'); os.exit(1)")?;

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    local....................................................................Failed
    - hook id: local
    - exit code: 1
      How are you

    ----- stderr -----
    "#);

    Ok(())
}

/// Test Lua script execution with file arguments.
#[test]
fn script_with_files() -> anyhow::Result<()> {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: lua
                name: lua
                language: lua
                entry: lua ./script.lua
                verbose: true
    "});

    context
        .work_dir()
        .child("script.lua")
        .write_str(indoc::indoc! {r#"
        for i, arg in ipairs(arg) do
            print("Processing file:", arg)
        end
    "#})?;

    context
        .work_dir()
        .child("test1.lua")
        .write_str("print('test1')")?;

    context
        .work_dir()
        .child("test2.lua")
        .write_str("print('test2')")?;

    context.git_add(".");

    let filters = context
        .filters()
        .into_iter()
        .chain([
            (r"Processing file:\s+(.+)", "Processing file: [FILENAME]"),
            (r"script\.lua", "[FILENAME]"),
            (r"\.pre-commit-config\.yaml", "[FILENAME]"),
            (r"test\d+\.lua", "[FILENAME]"),
        ])
        .collect::<Vec<_>>();

    cmd_snapshot!(filters, context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    lua......................................................................Passed
    - hook id: lua
    - duration: [TIME]
      Processing file: [FILENAME]
      Processing file: [FILENAME]
      Processing file: [FILENAME]
      Processing file: [FILENAME]

    ----- stderr -----
    "#);

    Ok(())
}

/// Test Lua environment variables (`LUA_PATH` and `LUA_CPATH`)
#[test]
fn lua_environment() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: lua
                name: lua
                language: lua
                entry: lua -e 'print("LUA_PATH:", os.getenv("LUA_PATH")); print("LUA_CPATH:", os.getenv("LUA_CPATH"))'
                always_run: true
                verbose: true
                pass_filenames: false
    "#});

    context.git_add(".");

    let filters = context
        .filters()
        .into_iter()
        .chain([
            (r"lua-[A-Za-z0-9]+", "lua-[HASH]"),
            (r"\t", " "), // Replace tabs with spaces
        ])
        .collect::<Vec<_>>();

    #[cfg(not(target_os = "windows"))]
    cmd_snapshot!(filters, context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    lua......................................................................Passed
    - hook id: lua
    - duration: [TIME]
      LUA_PATH: [HOME]/hooks/lua-[HASH]/share/lua/5.4/?.lua;[HOME]/hooks/lua-[HASH]/share/lua/5.4/?/init.lua
      LUA_CPATH: [HOME]/hooks/lua-[HASH]/lib/lua/5.4/?.so

    ----- stderr -----
    "#);
    #[cfg(target_os = "windows")]
    cmd_snapshot!(filters, context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    lua......................................................................Passed
    - hook id: lua
    - duration: [TIME]
      LUA_PATH: [HOME]/hooks/lua-[HASH]/share/lua/5.4\?.lua;[HOME]/hooks/lua-[HASH]/share/lua/5.4\?/init.lua
      LUA_CPATH: [HOME]/hooks/lua-[HASH]/lib/lua/5.4\?.dll

    ----- stderr -----
    "#);
}

/// Test Lua hook with complex dependencies.
#[test]
fn additional_dependencies() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: lua
                name: lua
                language: lua
                entry: lua -e 'require("lfs"); print("LuaFileSystem module loaded successfully")'
                additional_dependencies: ["luafilesystem"]
                always_run: true
                verbose: true
                pass_filenames: false
    "#});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    lua......................................................................Passed
    - hook id: lua
    - duration: [TIME]
      LuaFileSystem module loaded successfully

    ----- stderr -----
    "#);
}
