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

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    lua......................................................................Passed
    - hook id: lua
    - duration: [TIME]

      Hello from Lua!

    ----- stderr -----
    ");

    // Run again to check `health_check` works correctly.
    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    lua......................................................................Passed
    - hook id: lua
    - duration: [TIME]

      Hello from Lua!

    ----- stderr -----
    ");
}

/// Test specifying `language_version` for Lua hooks which is not supported for now.
#[test]
fn language_version() {
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
                language_version: '5.4'
                always_run: true
                verbose: true
                pass_filenames: false
    "});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to init hooks
      caused by: Invalid hook `local`
      caused by: Hook specified `language_version: 5.4` but the language `lua` does not support toolchain installation for now
    ");
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

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    local....................................................................Failed
    - hook id: local
    - exit code: 1

      How are you

    ----- stderr -----
    ");

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

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    lua......................................................................Passed
    - hook id: lua
    - duration: [TIME]

      Processing file:	script.lua
      Processing file:	.pre-commit-config.yaml
      Processing file:	test2.lua
      Processing file:	test1.lua

    ----- stderr -----
    ");

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
        .chain([(r"lua-[A-Za-z0-9]+", "lua-[HASH]")])
        .collect::<Vec<_>>();

    #[cfg(not(target_os = "windows"))]
    cmd_snapshot!(filters, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    lua......................................................................Passed
    - hook id: lua
    - duration: [TIME]

      LUA_PATH:	[HOME]/hooks/lua-[HASH]/share/lua/5.4/?.lua;[HOME]/hooks/lua-[HASH]/share/lua/5.4/?/init.lua;;
      LUA_CPATH:	[HOME]/hooks/lua-[HASH]/lib/lua/5.4/?.so;;

    ----- stderr -----
    ");

    #[cfg(target_os = "windows")]
    cmd_snapshot!(filters, context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    lua......................................................................Passed
    - hook id: lua
    - duration: [TIME]

      LUA_PATH:	[HOME]/hooks/lua-[HASH]/share/lua/5.4\?.lua;[HOME]/hooks/lua-[HASH]/share/lua/5.4\?/init.lua;;
      LUA_CPATH:	[HOME]/hooks/lua-[HASH]/lib/lua/5.4\?.dll;;

    ----- stderr -----
    "#);
}

/// Test Lua hook with additional dependencies.
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

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    lua......................................................................Passed
    - hook id: lua
    - duration: [TIME]

      LuaFileSystem module loaded successfully

    ----- stderr -----
    ");
}

/// Test remote Lua hook from GitHub repository.
#[test]
fn remote_hook() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: https://github.com/prek-test-repos/lua-hooks
            rev: v1.0.0
            hooks:
              - id: lua-hooks
                always_run: true
                verbose: true
    "});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    lua-hooks................................................................Passed
    - hook id: lua-hooks
    - duration: [TIME]

      this is a lua remote hook

    ----- stderr -----
    ");
}
