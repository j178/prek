# CLI Reference

## prek

Better pre-commit, re-engineered in Rust

<h3 class="cli-reference">Usage</h3>

```
prek [OPTIONS] [HOOK|PROJECT]... [COMMAND]
```

<h3 class="cli-reference">Commands</h3>

<dl class="cli-reference"><dt><a href="#prek-install"><code>prek install</code></a></dt><dd><p>Install the prek git hook</p></dd>
<dt><a href="#prek-install-hooks"><code>prek install-hooks</code></a></dt><dd><p>Create hook environments for all hooks used in the config file</p></dd>
<dt><a href="#prek-run"><code>prek run</code></a></dt><dd><p>Run hooks</p></dd>
<dt><a href="#prek-list"><code>prek list</code></a></dt><dd><p>List available hooks</p></dd>
<dt><a href="#prek-uninstall"><code>prek uninstall</code></a></dt><dd><p>Uninstall the prek git hook</p></dd>
<dt><a href="#prek-validate-config"><code>prek validate-config</code></a></dt><dd><p>Validate <code>.pre-commit-config.yaml</code> files</p></dd>
<dt><a href="#prek-validate-manifest"><code>prek validate-manifest</code></a></dt><dd><p>Validate <code>.pre-commit-hooks.yaml</code> files</p></dd>
<dt><a href="#prek-sample-config"><code>prek sample-config</code></a></dt><dd><p>Produce a sample <code>.pre-commit-config.yaml</code> file</p></dd>
<dt><a href="#prek-auto-update"><code>prek auto-update</code></a></dt><dd><p>Auto-update pre-commit config to the latest repos' versions</p></dd>
<dt><a href="#prek-cache"><code>prek cache</code></a></dt><dd><p>Manage the prek cache</p></dd>
<dt><a href="#prek-init-template-dir"><code>prek init-template-dir</code></a></dt><dd><p>Install hook script in a directory intended for use with <code>git config init.templateDir</code></p></dd>
<dt><a href="#prek-try-repo"><code>prek try-repo</code></a></dt><dd><p>Try the pre-commit hooks in the current repo</p></dd>
<dt><a href="#prek-self"><code>prek self</code></a></dt><dd><p><code>prek</code> self management</p></dd>
</dl>

## prek install

Install the prek git hook

<h3 class="cli-reference">Usage</h3>

```
prek install [OPTIONS] [HOOK|PROJECT]...
```

<h3 class="cli-reference">Arguments</h3>

<dl class="cli-reference"><dt id="prek-install--includes"><a href="#prek-install--includes"<code>HOOK|PROJECT</code></a></dt><dd><p>Include the specified hooks or projects.</p>
<p>Supports flexible selector syntax:</p>
<ul>
<li>
<p><code>hook-id</code>: Run all hooks with the specified ID across all projects</p>
</li>
<li>
<p><code>project-path/</code>: Run all hooks from the specified project</p>
</li>
<li>
<p><code>project-path:hook-id</code>: Run only the specified hook from the specified project</p>
</li>
</ul>
<p>Can be specified multiple times to select multiple hooks/projects.</p>
</dd></dl>

<h3 class="cli-reference">Options</h3>

<dl class="cli-reference"><dt id="prek-install--allow-missing-config"><a href="#prek-install--allow-missing-config"><code>--allow-missing-config</code></a></dt><dd><p>Allow a missing <code>pre-commit</code> configuration file</p>
</dd><dt id="prek-install--cd"><a href="#prek-install--cd"><code>--cd</code></a>, <code>-C</code> <i>dir</i></dt><dd><p>Change to directory before running</p>
</dd><dt id="prek-install--color"><a href="#prek-install--color"><code>--color</code></a> <i>color</i></dt><dd><p>Whether to use color in output</p>
<p>May also be set with the <code>PREK_COLOR</code> environment variable.</p><p>[default: auto]</p><p>Possible values:</p>
<ul>
<li><code>auto</code>:  Enables colored output only when the output is going to a terminal or TTY with support</li>
<li><code>always</code>:  Enables colored output regardless of the detected environment</li>
<li><code>never</code>:  Disables colored output</li>
</ul></dd><dt id="prek-install--config"><a href="#prek-install--config"><code>--config</code></a>, <code>-c</code> <i>config</i></dt><dd><p>Path to alternate config file</p>
</dd><dt id="prek-install--help"><a href="#prek-install--help"><code>--help</code></a>, <code>-h</code></dt><dd><p>Display the concise help for this command</p>
</dd><dt id="prek-install--hook-type"><a href="#prek-install--hook-type"><code>--hook-type</code></a>, <code>-t</code> <i>hook-type</i></dt><dt id="prek-install--install-hooks"><a href="#prek-install--install-hooks"><code>--install-hooks</code></a></dt><dd><p>Create hook environments for all hooks used in the config file</p>
</dd><dt id="prek-install--log-file"><a href="#prek-install--log-file"><code>--log-file</code></a> <i>log-file</i></dt><dd><p>Write trace logs to the specified file. If not specified, trace logs will be written to <code>$PREK_HOME/prek.log</code></p>
</dd><dt id="prek-install--no-progress"><a href="#prek-install--no-progress"><code>--no-progress</code></a></dt><dd><p>Hide all progress outputs.</p>
<p>For example, spinners or progress bars.</p>
</dd><dt id="prek-install--overwrite"><a href="#prek-install--overwrite"><code>--overwrite</code></a>, <code>-f</code></dt><dd><p>Overwrite existing hooks</p>
</dd><dt id="prek-install--quiet"><a href="#prek-install--quiet"><code>--quiet</code></a>, <code>-q</code></dt><dd><p>Use quiet output.</p>
<p>Repeating this option, e.g., <code>-qq</code>, will enable a silent mode in which prek will write no output to stdout.</p>
</dd><dt id="prek-install--refresh"><a href="#prek-install--refresh"><code>--refresh</code></a></dt><dd><p>Refresh all cached data</p>
</dd><dt id="prek-install--skip"><a href="#prek-install--skip"><code>--skip</code></a> <i>hook|project</i></dt><dd><p>Skip the specified hooks or projects.</p>
<p>Supports flexible selector syntax:</p>
<ul>
<li>
<p><code>hook-id</code>: Skip all hooks with the specified ID across all projects</p>
</li>
<li>
<p><code>project-path/</code>: Skip all hooks from the specified project</p>
</li>
<li>
<p><code>project-path:hook-id</code>: Skip only the specified hook from the specified project</p>
</li>
</ul>
<p>Can be specified multiple times. Also accepts <code>PREK_SKIP</code> or <code>SKIP</code> environment variables (comma-delimited).</p>
</dd><dt id="prek-install--verbose"><a href="#prek-install--verbose"><code>--verbose</code></a>, <code>-v</code></dt><dd><p>Use verbose output</p>
</dd><dt id="prek-install--version"><a href="#prek-install--version"><code>--version</code></a>, <code>-V</code></dt><dd><p>Display the prek version</p>
</dd></dl>

## prek install-hooks

Create hook environments for all hooks used in the config file.

This command does not install the git hook. To install the git hook along with the hook environments in one command, use `prek install --install-hooks`.

<h3 class="cli-reference">Usage</h3>

```
prek install-hooks [OPTIONS] [HOOK|PROJECT]...
```

<h3 class="cli-reference">Arguments</h3>

<dl class="cli-reference"><dt id="prek-install-hooks--includes"><a href="#prek-install-hooks--includes"<code>HOOK|PROJECT</code></a></dt><dd><p>Include the specified hooks or projects.</p>
<p>Supports flexible selector syntax:</p>
<ul>
<li>
<p><code>hook-id</code>: Run all hooks with the specified ID across all projects</p>
</li>
<li>
<p><code>project-path/</code>: Run all hooks from the specified project</p>
</li>
<li>
<p><code>project-path:hook-id</code>: Run only the specified hook from the specified project</p>
</li>
</ul>
<p>Can be specified multiple times to select multiple hooks/projects.</p>
</dd></dl>

<h3 class="cli-reference">Options</h3>

<dl class="cli-reference"><dt id="prek-install-hooks--cd"><a href="#prek-install-hooks--cd"><code>--cd</code></a>, <code>-C</code> <i>dir</i></dt><dd><p>Change to directory before running</p>
</dd><dt id="prek-install-hooks--color"><a href="#prek-install-hooks--color"><code>--color</code></a> <i>color</i></dt><dd><p>Whether to use color in output</p>
<p>May also be set with the <code>PREK_COLOR</code> environment variable.</p><p>[default: auto]</p><p>Possible values:</p>
<ul>
<li><code>auto</code>:  Enables colored output only when the output is going to a terminal or TTY with support</li>
<li><code>always</code>:  Enables colored output regardless of the detected environment</li>
<li><code>never</code>:  Disables colored output</li>
</ul></dd><dt id="prek-install-hooks--config"><a href="#prek-install-hooks--config"><code>--config</code></a>, <code>-c</code> <i>config</i></dt><dd><p>Path to alternate config file</p>
</dd><dt id="prek-install-hooks--help"><a href="#prek-install-hooks--help"><code>--help</code></a>, <code>-h</code></dt><dd><p>Display the concise help for this command</p>
</dd><dt id="prek-install-hooks--log-file"><a href="#prek-install-hooks--log-file"><code>--log-file</code></a> <i>log-file</i></dt><dd><p>Write trace logs to the specified file. If not specified, trace logs will be written to <code>$PREK_HOME/prek.log</code></p>
</dd><dt id="prek-install-hooks--no-progress"><a href="#prek-install-hooks--no-progress"><code>--no-progress</code></a></dt><dd><p>Hide all progress outputs.</p>
<p>For example, spinners or progress bars.</p>
</dd><dt id="prek-install-hooks--quiet"><a href="#prek-install-hooks--quiet"><code>--quiet</code></a>, <code>-q</code></dt><dd><p>Use quiet output.</p>
<p>Repeating this option, e.g., <code>-qq</code>, will enable a silent mode in which prek will write no output to stdout.</p>
</dd><dt id="prek-install-hooks--refresh"><a href="#prek-install-hooks--refresh"><code>--refresh</code></a></dt><dd><p>Refresh all cached data</p>
</dd><dt id="prek-install-hooks--skip"><a href="#prek-install-hooks--skip"><code>--skip</code></a> <i>hook|project</i></dt><dd><p>Skip the specified hooks or projects.</p>
<p>Supports flexible selector syntax:</p>
<ul>
<li>
<p><code>hook-id</code>: Skip all hooks with the specified ID across all projects</p>
</li>
<li>
<p><code>project-path/</code>: Skip all hooks from the specified project</p>
</li>
<li>
<p><code>project-path:hook-id</code>: Skip only the specified hook from the specified project</p>
</li>
</ul>
<p>Can be specified multiple times. Also accepts <code>PREK_SKIP</code> or <code>SKIP</code> environment variables (comma-delimited).</p>
</dd><dt id="prek-install-hooks--verbose"><a href="#prek-install-hooks--verbose"><code>--verbose</code></a>, <code>-v</code></dt><dd><p>Use verbose output</p>
</dd><dt id="prek-install-hooks--version"><a href="#prek-install-hooks--version"><code>--version</code></a>, <code>-V</code></dt><dd><p>Display the prek version</p>
</dd></dl>

## prek run

Run hooks

<h3 class="cli-reference">Usage</h3>

```
prek run [OPTIONS] [HOOK|PROJECT]...
```

<h3 class="cli-reference">Arguments</h3>

<dl class="cli-reference"><dt id="prek-run--includes"><a href="#prek-run--includes"<code>HOOK|PROJECT</code></a></dt><dd><p>Include the specified hooks or projects.</p>
<p>Supports flexible selector syntax:</p>
<ul>
<li>
<p><code>hook-id</code>: Run all hooks with the specified ID across all projects</p>
</li>
<li>
<p><code>project-path/</code>: Run all hooks from the specified project</p>
</li>
<li>
<p><code>project-path:hook-id</code>: Run only the specified hook from the specified project</p>
</li>
</ul>
<p>Can be specified multiple times to select multiple hooks/projects.</p>
</dd></dl>

<h3 class="cli-reference">Options</h3>

<dl class="cli-reference"><dt id="prek-run--all-files"><a href="#prek-run--all-files"><code>--all-files</code></a>, <code>-a</code></dt><dd><p>Run on all files in the repo</p>
</dd><dt id="prek-run--cd"><a href="#prek-run--cd"><code>--cd</code></a>, <code>-C</code> <i>dir</i></dt><dd><p>Change to directory before running</p>
</dd><dt id="prek-run--color"><a href="#prek-run--color"><code>--color</code></a> <i>color</i></dt><dd><p>Whether to use color in output</p>
<p>May also be set with the <code>PREK_COLOR</code> environment variable.</p><p>[default: auto]</p><p>Possible values:</p>
<ul>
<li><code>auto</code>:  Enables colored output only when the output is going to a terminal or TTY with support</li>
<li><code>always</code>:  Enables colored output regardless of the detected environment</li>
<li><code>never</code>:  Disables colored output</li>
</ul></dd><dt id="prek-run--config"><a href="#prek-run--config"><code>--config</code></a>, <code>-c</code> <i>config</i></dt><dd><p>Path to alternate config file</p>
</dd><dt id="prek-run--directory"><a href="#prek-run--directory"><code>--directory</code></a>, <code>-d</code> <i>dir</i></dt><dd><p>Run hooks on all files in the specified directories.</p>
<p>You can specify multiple directories. It can be used in conjunction with <code>--files</code>.</p>
</dd><dt id="prek-run--dry-run"><a href="#prek-run--dry-run"><code>--dry-run</code></a></dt><dd><p>Do not run the hooks, but print the hooks that would have been run</p>
</dd><dt id="prek-run--fail-fast"><a href="#prek-run--fail-fast"><code>--fail-fast</code></a></dt><dd><p>Stop running hooks after the first failure</p>
</dd><dt id="prek-run--files"><a href="#prek-run--files"><code>--files</code></a> <i>files</i></dt><dd><p>Specific filenames to run hooks on</p>
</dd><dt id="prek-run--from-ref"><a href="#prek-run--from-ref"><code>--from-ref</code></a>, <code>--source</code>, <code>-s</code> <i>from-ref</i></dt><dd><p>The original ref in a <code>&lt;from_ref&gt;...&lt;to_ref&gt;</code> diff expression. Files changed in this diff will be run through the hooks</p>
</dd><dt id="prek-run--help"><a href="#prek-run--help"><code>--help</code></a>, <code>-h</code></dt><dd><p>Display the concise help for this command</p>
</dd><dt id="prek-run--hook-stage"><a href="#prek-run--hook-stage"><code>--hook-stage</code></a> <i>hook-stage</i></dt><dd><p>The stage during which the hook is fired.</p>
<p>When specified, only hooks configured for that stage (for example <code>manual</code>, <code>pre-commit</code>, or <code>pre-commit</code>) will run. Defaults to <code>pre-commit</code> if not specified. For hooks specified directly in the command line, fallback to <code>manual</code> stage if no hooks found for <code>pre-commit</code> stage.</p>
<p>Possible values:</p>
<ul>
<li><code>manual</code></li>
<li><code>commit-msg</code></li>
<li><code>post-checkout</code></li>
<li><code>post-commit</code></li>
<li><code>post-merge</code></li>
<li><code>post-rewrite</code></li>
<li><code>pre-commit</code></li>
<li><code>pre-merge-commit</code></li>
<li><code>pre-push</code></li>
<li><code>pre-rebase</code></li>
<li><code>prepare-commit-msg</code></li>
</ul></dd><dt id="prek-run--last-commit"><a href="#prek-run--last-commit"><code>--last-commit</code></a></dt><dd><p>Run hooks against the last commit. Equivalent to <code>--from-ref HEAD~1 --to-ref HEAD</code></p>
</dd><dt id="prek-run--log-file"><a href="#prek-run--log-file"><code>--log-file</code></a> <i>log-file</i></dt><dd><p>Write trace logs to the specified file. If not specified, trace logs will be written to <code>$PREK_HOME/prek.log</code></p>
</dd><dt id="prek-run--no-progress"><a href="#prek-run--no-progress"><code>--no-progress</code></a></dt><dd><p>Hide all progress outputs.</p>
<p>For example, spinners or progress bars.</p>
</dd><dt id="prek-run--quiet"><a href="#prek-run--quiet"><code>--quiet</code></a>, <code>-q</code></dt><dd><p>Use quiet output.</p>
<p>Repeating this option, e.g., <code>-qq</code>, will enable a silent mode in which prek will write no output to stdout.</p>
</dd><dt id="prek-run--refresh"><a href="#prek-run--refresh"><code>--refresh</code></a></dt><dd><p>Refresh all cached data</p>
</dd><dt id="prek-run--show-diff-on-failure"><a href="#prek-run--show-diff-on-failure"><code>--show-diff-on-failure</code></a></dt><dd><p>When hooks fail, run <code>git diff</code> directly afterward</p>
</dd><dt id="prek-run--skip"><a href="#prek-run--skip"><code>--skip</code></a> <i>hook|project</i></dt><dd><p>Skip the specified hooks or projects.</p>
<p>Supports flexible selector syntax:</p>
<ul>
<li>
<p><code>hook-id</code>: Skip all hooks with the specified ID across all projects</p>
</li>
<li>
<p><code>project-path/</code>: Skip all hooks from the specified project</p>
</li>
<li>
<p><code>project-path:hook-id</code>: Skip only the specified hook from the specified project</p>
</li>
</ul>
<p>Can be specified multiple times. Also accepts <code>PREK_SKIP</code> or <code>SKIP</code> environment variables (comma-delimited).</p>
</dd><dt id="prek-run--to-ref"><a href="#prek-run--to-ref"><code>--to-ref</code></a>, <code>--origin</code>, <code>-o</code> <i>to-ref</i></dt><dd><p>The destination ref in a <code>from_ref...to_ref</code> diff expression. Defaults to <code>HEAD</code> if <code>from_ref</code> is specified</p>
</dd><dt id="prek-run--verbose"><a href="#prek-run--verbose"><code>--verbose</code></a>, <code>-v</code></dt><dd><p>Use verbose output</p>
</dd><dt id="prek-run--version"><a href="#prek-run--version"><code>--version</code></a>, <code>-V</code></dt><dd><p>Display the prek version</p>
</dd></dl>

## prek list

List available hooks

<h3 class="cli-reference">Usage</h3>

```
prek list [OPTIONS] [HOOK|PROJECT]...
```

<h3 class="cli-reference">Arguments</h3>

<dl class="cli-reference"><dt id="prek-list--includes"><a href="#prek-list--includes"<code>HOOK|PROJECT</code></a></dt><dd><p>Include the specified hooks or projects.</p>
<p>Supports flexible selector syntax:</p>
<ul>
<li>
<p><code>hook-id</code>: Run all hooks with the specified ID across all projects</p>
</li>
<li>
<p><code>project-path/</code>: Run all hooks from the specified project</p>
</li>
<li>
<p><code>project-path:hook-id</code>: Run only the specified hook from the specified project</p>
</li>
</ul>
<p>Can be specified multiple times to select multiple hooks/projects.</p>
</dd></dl>

<h3 class="cli-reference">Options</h3>

<dl class="cli-reference"><dt id="prek-list--cd"><a href="#prek-list--cd"><code>--cd</code></a>, <code>-C</code> <i>dir</i></dt><dd><p>Change to directory before running</p>
</dd><dt id="prek-list--color"><a href="#prek-list--color"><code>--color</code></a> <i>color</i></dt><dd><p>Whether to use color in output</p>
<p>May also be set with the <code>PREK_COLOR</code> environment variable.</p><p>[default: auto]</p><p>Possible values:</p>
<ul>
<li><code>auto</code>:  Enables colored output only when the output is going to a terminal or TTY with support</li>
<li><code>always</code>:  Enables colored output regardless of the detected environment</li>
<li><code>never</code>:  Disables colored output</li>
</ul></dd><dt id="prek-list--config"><a href="#prek-list--config"><code>--config</code></a>, <code>-c</code> <i>config</i></dt><dd><p>Path to alternate config file</p>
</dd><dt id="prek-list--help"><a href="#prek-list--help"><code>--help</code></a>, <code>-h</code></dt><dd><p>Display the concise help for this command</p>
</dd><dt id="prek-list--hook-stage"><a href="#prek-list--hook-stage"><code>--hook-stage</code></a> <i>hook-stage</i></dt><dd><p>Show only hooks that has the specified stage</p>
<p>Possible values:</p>
<ul>
<li><code>manual</code></li>
<li><code>commit-msg</code></li>
<li><code>post-checkout</code></li>
<li><code>post-commit</code></li>
<li><code>post-merge</code></li>
<li><code>post-rewrite</code></li>
<li><code>pre-commit</code></li>
<li><code>pre-merge-commit</code></li>
<li><code>pre-push</code></li>
<li><code>pre-rebase</code></li>
<li><code>prepare-commit-msg</code></li>
</ul></dd><dt id="prek-list--language"><a href="#prek-list--language"><code>--language</code></a> <i>language</i></dt><dd><p>Show only hooks that are implemented in the specified language</p>
<p>Possible values:</p>
<ul>
<li><code>conda</code></li>
<li><code>coursier</code></li>
<li><code>dart</code></li>
<li><code>docker</code></li>
<li><code>docker-image</code></li>
<li><code>dotnet</code></li>
<li><code>fail</code></li>
<li><code>golang</code></li>
<li><code>haskell</code></li>
<li><code>lua</code></li>
<li><code>node</code></li>
<li><code>perl</code></li>
<li><code>python</code></li>
<li><code>r</code></li>
<li><code>ruby</code></li>
<li><code>rust</code></li>
<li><code>swift</code></li>
<li><code>pygrep</code></li>
<li><code>script</code></li>
<li><code>system</code></li>
</ul></dd><dt id="prek-list--log-file"><a href="#prek-list--log-file"><code>--log-file</code></a> <i>log-file</i></dt><dd><p>Write trace logs to the specified file. If not specified, trace logs will be written to <code>$PREK_HOME/prek.log</code></p>
</dd><dt id="prek-list--no-progress"><a href="#prek-list--no-progress"><code>--no-progress</code></a></dt><dd><p>Hide all progress outputs.</p>
<p>For example, spinners or progress bars.</p>
</dd><dt id="prek-list--output-format"><a href="#prek-list--output-format"><code>--output-format</code></a> <i>output-format</i></dt><dd><p>The output format</p>
<p>[default: text]</p><p>Possible values:</p>
<ul>
<li><code>text</code></li>
<li><code>json</code></li>
</ul></dd><dt id="prek-list--quiet"><a href="#prek-list--quiet"><code>--quiet</code></a>, <code>-q</code></dt><dd><p>Use quiet output.</p>
<p>Repeating this option, e.g., <code>-qq</code>, will enable a silent mode in which prek will write no output to stdout.</p>
</dd><dt id="prek-list--refresh"><a href="#prek-list--refresh"><code>--refresh</code></a></dt><dd><p>Refresh all cached data</p>
</dd><dt id="prek-list--skip"><a href="#prek-list--skip"><code>--skip</code></a> <i>hook|project</i></dt><dd><p>Skip the specified hooks or projects.</p>
<p>Supports flexible selector syntax:</p>
<ul>
<li>
<p><code>hook-id</code>: Skip all hooks with the specified ID across all projects</p>
</li>
<li>
<p><code>project-path/</code>: Skip all hooks from the specified project</p>
</li>
<li>
<p><code>project-path:hook-id</code>: Skip only the specified hook from the specified project</p>
</li>
</ul>
<p>Can be specified multiple times. Also accepts <code>PREK_SKIP</code> or <code>SKIP</code> environment variables (comma-delimited).</p>
</dd><dt id="prek-list--verbose"><a href="#prek-list--verbose"><code>--verbose</code></a>, <code>-v</code></dt><dd><p>Use verbose output</p>
</dd><dt id="prek-list--version"><a href="#prek-list--version"><code>--version</code></a>, <code>-V</code></dt><dd><p>Display the prek version</p>
</dd></dl>

## prek uninstall

Uninstall the prek git hook

<h3 class="cli-reference">Usage</h3>

```
prek uninstall [OPTIONS]
```

<h3 class="cli-reference">Options</h3>

<dl class="cli-reference"><dt id="prek-uninstall--cd"><a href="#prek-uninstall--cd"><code>--cd</code></a>, <code>-C</code> <i>dir</i></dt><dd><p>Change to directory before running</p>
</dd><dt id="prek-uninstall--color"><a href="#prek-uninstall--color"><code>--color</code></a> <i>color</i></dt><dd><p>Whether to use color in output</p>
<p>May also be set with the <code>PREK_COLOR</code> environment variable.</p><p>[default: auto]</p><p>Possible values:</p>
<ul>
<li><code>auto</code>:  Enables colored output only when the output is going to a terminal or TTY with support</li>
<li><code>always</code>:  Enables colored output regardless of the detected environment</li>
<li><code>never</code>:  Disables colored output</li>
</ul></dd><dt id="prek-uninstall--config"><a href="#prek-uninstall--config"><code>--config</code></a>, <code>-c</code> <i>config</i></dt><dd><p>Path to alternate config file</p>
</dd><dt id="prek-uninstall--help"><a href="#prek-uninstall--help"><code>--help</code></a>, <code>-h</code></dt><dd><p>Display the concise help for this command</p>
</dd><dt id="prek-uninstall--hook-type"><a href="#prek-uninstall--hook-type"><code>--hook-type</code></a>, <code>-t</code> <i>hook-type</i></dt><dt id="prek-uninstall--log-file"><a href="#prek-uninstall--log-file"><code>--log-file</code></a> <i>log-file</i></dt><dd><p>Write trace logs to the specified file. If not specified, trace logs will be written to <code>$PREK_HOME/prek.log</code></p>
</dd><dt id="prek-uninstall--no-progress"><a href="#prek-uninstall--no-progress"><code>--no-progress</code></a></dt><dd><p>Hide all progress outputs.</p>
<p>For example, spinners or progress bars.</p>
</dd><dt id="prek-uninstall--quiet"><a href="#prek-uninstall--quiet"><code>--quiet</code></a>, <code>-q</code></dt><dd><p>Use quiet output.</p>
<p>Repeating this option, e.g., <code>-qq</code>, will enable a silent mode in which prek will write no output to stdout.</p>
</dd><dt id="prek-uninstall--refresh"><a href="#prek-uninstall--refresh"><code>--refresh</code></a></dt><dd><p>Refresh all cached data</p>
</dd><dt id="prek-uninstall--verbose"><a href="#prek-uninstall--verbose"><code>--verbose</code></a>, <code>-v</code></dt><dd><p>Use verbose output</p>
</dd><dt id="prek-uninstall--version"><a href="#prek-uninstall--version"><code>--version</code></a>, <code>-V</code></dt><dd><p>Display the prek version</p>
</dd></dl>

## prek validate-config

Validate `.pre-commit-config.yaml` files

<h3 class="cli-reference">Usage</h3>

```
prek validate-config [OPTIONS] [CONFIG]...
```

<h3 class="cli-reference">Arguments</h3>

<dl class="cli-reference"><dt id="prek-validate-config--configs"><a href="#prek-validate-config--configs"<code>CONFIG</code></a></dt><dd><p>The path to the configuration file</p>
</dd></dl>

<h3 class="cli-reference">Options</h3>

<dl class="cli-reference"><dt id="prek-validate-config--cd"><a href="#prek-validate-config--cd"><code>--cd</code></a>, <code>-C</code> <i>dir</i></dt><dd><p>Change to directory before running</p>
</dd><dt id="prek-validate-config--color"><a href="#prek-validate-config--color"><code>--color</code></a> <i>color</i></dt><dd><p>Whether to use color in output</p>
<p>May also be set with the <code>PREK_COLOR</code> environment variable.</p><p>[default: auto]</p><p>Possible values:</p>
<ul>
<li><code>auto</code>:  Enables colored output only when the output is going to a terminal or TTY with support</li>
<li><code>always</code>:  Enables colored output regardless of the detected environment</li>
<li><code>never</code>:  Disables colored output</li>
</ul></dd><dt id="prek-validate-config--config"><a href="#prek-validate-config--config"><code>--config</code></a>, <code>-c</code> <i>config</i></dt><dd><p>Path to alternate config file</p>
</dd><dt id="prek-validate-config--help"><a href="#prek-validate-config--help"><code>--help</code></a>, <code>-h</code></dt><dd><p>Display the concise help for this command</p>
</dd><dt id="prek-validate-config--log-file"><a href="#prek-validate-config--log-file"><code>--log-file</code></a> <i>log-file</i></dt><dd><p>Write trace logs to the specified file. If not specified, trace logs will be written to <code>$PREK_HOME/prek.log</code></p>
</dd><dt id="prek-validate-config--no-progress"><a href="#prek-validate-config--no-progress"><code>--no-progress</code></a></dt><dd><p>Hide all progress outputs.</p>
<p>For example, spinners or progress bars.</p>
</dd><dt id="prek-validate-config--quiet"><a href="#prek-validate-config--quiet"><code>--quiet</code></a>, <code>-q</code></dt><dd><p>Use quiet output.</p>
<p>Repeating this option, e.g., <code>-qq</code>, will enable a silent mode in which prek will write no output to stdout.</p>
</dd><dt id="prek-validate-config--refresh"><a href="#prek-validate-config--refresh"><code>--refresh</code></a></dt><dd><p>Refresh all cached data</p>
</dd><dt id="prek-validate-config--verbose"><a href="#prek-validate-config--verbose"><code>--verbose</code></a>, <code>-v</code></dt><dd><p>Use verbose output</p>
</dd><dt id="prek-validate-config--version"><a href="#prek-validate-config--version"><code>--version</code></a>, <code>-V</code></dt><dd><p>Display the prek version</p>
</dd></dl>

## prek validate-manifest

Validate `.pre-commit-hooks.yaml` files

<h3 class="cli-reference">Usage</h3>

```
prek validate-manifest [OPTIONS] [MANIFEST]...
```

<h3 class="cli-reference">Arguments</h3>

<dl class="cli-reference"><dt id="prek-validate-manifest--manifests"><a href="#prek-validate-manifest--manifests"<code>MANIFEST</code></a></dt><dd><p>The path to the manifest file</p>
</dd></dl>

<h3 class="cli-reference">Options</h3>

<dl class="cli-reference"><dt id="prek-validate-manifest--cd"><a href="#prek-validate-manifest--cd"><code>--cd</code></a>, <code>-C</code> <i>dir</i></dt><dd><p>Change to directory before running</p>
</dd><dt id="prek-validate-manifest--color"><a href="#prek-validate-manifest--color"><code>--color</code></a> <i>color</i></dt><dd><p>Whether to use color in output</p>
<p>May also be set with the <code>PREK_COLOR</code> environment variable.</p><p>[default: auto]</p><p>Possible values:</p>
<ul>
<li><code>auto</code>:  Enables colored output only when the output is going to a terminal or TTY with support</li>
<li><code>always</code>:  Enables colored output regardless of the detected environment</li>
<li><code>never</code>:  Disables colored output</li>
</ul></dd><dt id="prek-validate-manifest--config"><a href="#prek-validate-manifest--config"><code>--config</code></a>, <code>-c</code> <i>config</i></dt><dd><p>Path to alternate config file</p>
</dd><dt id="prek-validate-manifest--help"><a href="#prek-validate-manifest--help"><code>--help</code></a>, <code>-h</code></dt><dd><p>Display the concise help for this command</p>
</dd><dt id="prek-validate-manifest--log-file"><a href="#prek-validate-manifest--log-file"><code>--log-file</code></a> <i>log-file</i></dt><dd><p>Write trace logs to the specified file. If not specified, trace logs will be written to <code>$PREK_HOME/prek.log</code></p>
</dd><dt id="prek-validate-manifest--no-progress"><a href="#prek-validate-manifest--no-progress"><code>--no-progress</code></a></dt><dd><p>Hide all progress outputs.</p>
<p>For example, spinners or progress bars.</p>
</dd><dt id="prek-validate-manifest--quiet"><a href="#prek-validate-manifest--quiet"><code>--quiet</code></a>, <code>-q</code></dt><dd><p>Use quiet output.</p>
<p>Repeating this option, e.g., <code>-qq</code>, will enable a silent mode in which prek will write no output to stdout.</p>
</dd><dt id="prek-validate-manifest--refresh"><a href="#prek-validate-manifest--refresh"><code>--refresh</code></a></dt><dd><p>Refresh all cached data</p>
</dd><dt id="prek-validate-manifest--verbose"><a href="#prek-validate-manifest--verbose"><code>--verbose</code></a>, <code>-v</code></dt><dd><p>Use verbose output</p>
</dd><dt id="prek-validate-manifest--version"><a href="#prek-validate-manifest--version"><code>--version</code></a>, <code>-V</code></dt><dd><p>Display the prek version</p>
</dd></dl>

## prek sample-config

Produce a sample `.pre-commit-config.yaml` file

<h3 class="cli-reference">Usage</h3>

```
prek sample-config [OPTIONS]
```

<h3 class="cli-reference">Options</h3>

<dl class="cli-reference"><dt id="prek-sample-config--cd"><a href="#prek-sample-config--cd"><code>--cd</code></a>, <code>-C</code> <i>dir</i></dt><dd><p>Change to directory before running</p>
</dd><dt id="prek-sample-config--color"><a href="#prek-sample-config--color"><code>--color</code></a> <i>color</i></dt><dd><p>Whether to use color in output</p>
<p>May also be set with the <code>PREK_COLOR</code> environment variable.</p><p>[default: auto]</p><p>Possible values:</p>
<ul>
<li><code>auto</code>:  Enables colored output only when the output is going to a terminal or TTY with support</li>
<li><code>always</code>:  Enables colored output regardless of the detected environment</li>
<li><code>never</code>:  Disables colored output</li>
</ul></dd><dt id="prek-sample-config--config"><a href="#prek-sample-config--config"><code>--config</code></a>, <code>-c</code> <i>config</i></dt><dd><p>Path to alternate config file</p>
</dd><dt id="prek-sample-config--file"><a href="#prek-sample-config--file"><code>--file</code></a>, <code>-f</code> <i>file</i></dt><dd><p>Write the sample config to a file (<code>.pre-commit-config.yaml</code> by default)</p>
</dd><dt id="prek-sample-config--help"><a href="#prek-sample-config--help"><code>--help</code></a>, <code>-h</code></dt><dd><p>Display the concise help for this command</p>
</dd><dt id="prek-sample-config--log-file"><a href="#prek-sample-config--log-file"><code>--log-file</code></a> <i>log-file</i></dt><dd><p>Write trace logs to the specified file. If not specified, trace logs will be written to <code>$PREK_HOME/prek.log</code></p>
</dd><dt id="prek-sample-config--no-progress"><a href="#prek-sample-config--no-progress"><code>--no-progress</code></a></dt><dd><p>Hide all progress outputs.</p>
<p>For example, spinners or progress bars.</p>
</dd><dt id="prek-sample-config--quiet"><a href="#prek-sample-config--quiet"><code>--quiet</code></a>, <code>-q</code></dt><dd><p>Use quiet output.</p>
<p>Repeating this option, e.g., <code>-qq</code>, will enable a silent mode in which prek will write no output to stdout.</p>
</dd><dt id="prek-sample-config--refresh"><a href="#prek-sample-config--refresh"><code>--refresh</code></a></dt><dd><p>Refresh all cached data</p>
</dd><dt id="prek-sample-config--verbose"><a href="#prek-sample-config--verbose"><code>--verbose</code></a>, <code>-v</code></dt><dd><p>Use verbose output</p>
</dd><dt id="prek-sample-config--version"><a href="#prek-sample-config--version"><code>--version</code></a>, <code>-V</code></dt><dd><p>Display the prek version</p>
</dd></dl>

## prek auto-update

Auto-update pre-commit config to the latest repos' versions

<h3 class="cli-reference">Usage</h3>

```
prek auto-update [OPTIONS]
```

<h3 class="cli-reference">Options</h3>

<dl class="cli-reference"><dt id="prek-auto-update--bleeding-edge"><a href="#prek-auto-update--bleeding-edge"><code>--bleeding-edge</code></a></dt><dd><p>Update to the bleeding edge of the default branch instead of the latest tagged version</p>
</dd><dt id="prek-auto-update--cd"><a href="#prek-auto-update--cd"><code>--cd</code></a>, <code>-C</code> <i>dir</i></dt><dd><p>Change to directory before running</p>
</dd><dt id="prek-auto-update--color"><a href="#prek-auto-update--color"><code>--color</code></a> <i>color</i></dt><dd><p>Whether to use color in output</p>
<p>May also be set with the <code>PREK_COLOR</code> environment variable.</p><p>[default: auto]</p><p>Possible values:</p>
<ul>
<li><code>auto</code>:  Enables colored output only when the output is going to a terminal or TTY with support</li>
<li><code>always</code>:  Enables colored output regardless of the detected environment</li>
<li><code>never</code>:  Disables colored output</li>
</ul></dd><dt id="prek-auto-update--config"><a href="#prek-auto-update--config"><code>--config</code></a>, <code>-c</code> <i>config</i></dt><dd><p>Path to alternate config file</p>
</dd><dt id="prek-auto-update--dry-run"><a href="#prek-auto-update--dry-run"><code>--dry-run</code></a></dt><dd><p>Do not write changes to the config file, only display what would be changed</p>
</dd><dt id="prek-auto-update--freeze"><a href="#prek-auto-update--freeze"><code>--freeze</code></a></dt><dd><p>Store &quot;frozen&quot; hashes in <code>rev</code> instead of tag names</p>
</dd><dt id="prek-auto-update--help"><a href="#prek-auto-update--help"><code>--help</code></a>, <code>-h</code></dt><dd><p>Display the concise help for this command</p>
</dd><dt id="prek-auto-update--jobs"><a href="#prek-auto-update--jobs"><code>--jobs</code></a>, <code>-j</code> <i>jobs</i></dt><dd><p>Number of threads to use</p>
<p>[default: 0]</p></dd><dt id="prek-auto-update--log-file"><a href="#prek-auto-update--log-file"><code>--log-file</code></a> <i>log-file</i></dt><dd><p>Write trace logs to the specified file. If not specified, trace logs will be written to <code>$PREK_HOME/prek.log</code></p>
</dd><dt id="prek-auto-update--no-progress"><a href="#prek-auto-update--no-progress"><code>--no-progress</code></a></dt><dd><p>Hide all progress outputs.</p>
<p>For example, spinners or progress bars.</p>
</dd><dt id="prek-auto-update--quiet"><a href="#prek-auto-update--quiet"><code>--quiet</code></a>, <code>-q</code></dt><dd><p>Use quiet output.</p>
<p>Repeating this option, e.g., <code>-qq</code>, will enable a silent mode in which prek will write no output to stdout.</p>
</dd><dt id="prek-auto-update--refresh"><a href="#prek-auto-update--refresh"><code>--refresh</code></a></dt><dd><p>Refresh all cached data</p>
</dd><dt id="prek-auto-update--repo"><a href="#prek-auto-update--repo"><code>--repo</code></a> <i>repo</i></dt><dd><p>Only update this repository. This option may be specified multiple times</p>
</dd><dt id="prek-auto-update--verbose"><a href="#prek-auto-update--verbose"><code>--verbose</code></a>, <code>-v</code></dt><dd><p>Use verbose output</p>
</dd><dt id="prek-auto-update--version"><a href="#prek-auto-update--version"><code>--version</code></a>, <code>-V</code></dt><dd><p>Display the prek version</p>
</dd></dl>

## prek cache

Manage the prek cache

<h3 class="cli-reference">Usage</h3>

```
prek cache [OPTIONS] <COMMAND>
```

<h3 class="cli-reference">Commands</h3>

<dl class="cli-reference"><dt><a href="#prek-cache-dir"><code>prek cache dir</code></a></dt><dd><p>Show the location of the prek cache</p></dd>
<dt><a href="#prek-cache-gc"><code>prek cache gc</code></a></dt><dd><p>Remove unused cached repositories, hook environments, and other data</p></dd>
<dt><a href="#prek-cache-clean"><code>prek cache clean</code></a></dt><dd><p>Remove all prek cached data</p></dd>
<dt><a href="#prek-cache-size"><code>prek cache size</code></a></dt><dd><p>Show the size of the prek cache</p></dd>
</dl>

### prek cache dir

Show the location of the prek cache

<h3 class="cli-reference">Usage</h3>

```
prek cache dir [OPTIONS]
```

<h3 class="cli-reference">Options</h3>

<dl class="cli-reference"><dt id="prek-cache-dir--cd"><a href="#prek-cache-dir--cd"><code>--cd</code></a>, <code>-C</code> <i>dir</i></dt><dd><p>Change to directory before running</p>
</dd><dt id="prek-cache-dir--color"><a href="#prek-cache-dir--color"><code>--color</code></a> <i>color</i></dt><dd><p>Whether to use color in output</p>
<p>May also be set with the <code>PREK_COLOR</code> environment variable.</p><p>[default: auto]</p><p>Possible values:</p>
<ul>
<li><code>auto</code>:  Enables colored output only when the output is going to a terminal or TTY with support</li>
<li><code>always</code>:  Enables colored output regardless of the detected environment</li>
<li><code>never</code>:  Disables colored output</li>
</ul></dd><dt id="prek-cache-dir--config"><a href="#prek-cache-dir--config"><code>--config</code></a>, <code>-c</code> <i>config</i></dt><dd><p>Path to alternate config file</p>
</dd><dt id="prek-cache-dir--help"><a href="#prek-cache-dir--help"><code>--help</code></a>, <code>-h</code></dt><dd><p>Display the concise help for this command</p>
</dd><dt id="prek-cache-dir--log-file"><a href="#prek-cache-dir--log-file"><code>--log-file</code></a> <i>log-file</i></dt><dd><p>Write trace logs to the specified file. If not specified, trace logs will be written to <code>$PREK_HOME/prek.log</code></p>
</dd><dt id="prek-cache-dir--no-progress"><a href="#prek-cache-dir--no-progress"><code>--no-progress</code></a></dt><dd><p>Hide all progress outputs.</p>
<p>For example, spinners or progress bars.</p>
</dd><dt id="prek-cache-dir--quiet"><a href="#prek-cache-dir--quiet"><code>--quiet</code></a>, <code>-q</code></dt><dd><p>Use quiet output.</p>
<p>Repeating this option, e.g., <code>-qq</code>, will enable a silent mode in which prek will write no output to stdout.</p>
</dd><dt id="prek-cache-dir--refresh"><a href="#prek-cache-dir--refresh"><code>--refresh</code></a></dt><dd><p>Refresh all cached data</p>
</dd><dt id="prek-cache-dir--verbose"><a href="#prek-cache-dir--verbose"><code>--verbose</code></a>, <code>-v</code></dt><dd><p>Use verbose output</p>
</dd><dt id="prek-cache-dir--version"><a href="#prek-cache-dir--version"><code>--version</code></a>, <code>-V</code></dt><dd><p>Display the prek version</p>
</dd></dl>

### prek cache gc

Remove unused cached repositories, hook environments, and other data

<h3 class="cli-reference">Usage</h3>

```
prek cache gc [OPTIONS]
```

<h3 class="cli-reference">Options</h3>

<dl class="cli-reference"><dt id="prek-cache-gc--cd"><a href="#prek-cache-gc--cd"><code>--cd</code></a>, <code>-C</code> <i>dir</i></dt><dd><p>Change to directory before running</p>
</dd><dt id="prek-cache-gc--color"><a href="#prek-cache-gc--color"><code>--color</code></a> <i>color</i></dt><dd><p>Whether to use color in output</p>
<p>May also be set with the <code>PREK_COLOR</code> environment variable.</p><p>[default: auto]</p><p>Possible values:</p>
<ul>
<li><code>auto</code>:  Enables colored output only when the output is going to a terminal or TTY with support</li>
<li><code>always</code>:  Enables colored output regardless of the detected environment</li>
<li><code>never</code>:  Disables colored output</li>
</ul></dd><dt id="prek-cache-gc--config"><a href="#prek-cache-gc--config"><code>--config</code></a>, <code>-c</code> <i>config</i></dt><dd><p>Path to alternate config file</p>
</dd><dt id="prek-cache-gc--help"><a href="#prek-cache-gc--help"><code>--help</code></a>, <code>-h</code></dt><dd><p>Display the concise help for this command</p>
</dd><dt id="prek-cache-gc--log-file"><a href="#prek-cache-gc--log-file"><code>--log-file</code></a> <i>log-file</i></dt><dd><p>Write trace logs to the specified file. If not specified, trace logs will be written to <code>$PREK_HOME/prek.log</code></p>
</dd><dt id="prek-cache-gc--no-progress"><a href="#prek-cache-gc--no-progress"><code>--no-progress</code></a></dt><dd><p>Hide all progress outputs.</p>
<p>For example, spinners or progress bars.</p>
</dd><dt id="prek-cache-gc--quiet"><a href="#prek-cache-gc--quiet"><code>--quiet</code></a>, <code>-q</code></dt><dd><p>Use quiet output.</p>
<p>Repeating this option, e.g., <code>-qq</code>, will enable a silent mode in which prek will write no output to stdout.</p>
</dd><dt id="prek-cache-gc--refresh"><a href="#prek-cache-gc--refresh"><code>--refresh</code></a></dt><dd><p>Refresh all cached data</p>
</dd><dt id="prek-cache-gc--verbose"><a href="#prek-cache-gc--verbose"><code>--verbose</code></a>, <code>-v</code></dt><dd><p>Use verbose output</p>
</dd><dt id="prek-cache-gc--version"><a href="#prek-cache-gc--version"><code>--version</code></a>, <code>-V</code></dt><dd><p>Display the prek version</p>
</dd></dl>

### prek cache clean

Remove all prek cached data

<h3 class="cli-reference">Usage</h3>

```
prek cache clean [OPTIONS]
```

<h3 class="cli-reference">Options</h3>

<dl class="cli-reference"><dt id="prek-cache-clean--cd"><a href="#prek-cache-clean--cd"><code>--cd</code></a>, <code>-C</code> <i>dir</i></dt><dd><p>Change to directory before running</p>
</dd><dt id="prek-cache-clean--color"><a href="#prek-cache-clean--color"><code>--color</code></a> <i>color</i></dt><dd><p>Whether to use color in output</p>
<p>May also be set with the <code>PREK_COLOR</code> environment variable.</p><p>[default: auto]</p><p>Possible values:</p>
<ul>
<li><code>auto</code>:  Enables colored output only when the output is going to a terminal or TTY with support</li>
<li><code>always</code>:  Enables colored output regardless of the detected environment</li>
<li><code>never</code>:  Disables colored output</li>
</ul></dd><dt id="prek-cache-clean--config"><a href="#prek-cache-clean--config"><code>--config</code></a>, <code>-c</code> <i>config</i></dt><dd><p>Path to alternate config file</p>
</dd><dt id="prek-cache-clean--help"><a href="#prek-cache-clean--help"><code>--help</code></a>, <code>-h</code></dt><dd><p>Display the concise help for this command</p>
</dd><dt id="prek-cache-clean--log-file"><a href="#prek-cache-clean--log-file"><code>--log-file</code></a> <i>log-file</i></dt><dd><p>Write trace logs to the specified file. If not specified, trace logs will be written to <code>$PREK_HOME/prek.log</code></p>
</dd><dt id="prek-cache-clean--no-progress"><a href="#prek-cache-clean--no-progress"><code>--no-progress</code></a></dt><dd><p>Hide all progress outputs.</p>
<p>For example, spinners or progress bars.</p>
</dd><dt id="prek-cache-clean--quiet"><a href="#prek-cache-clean--quiet"><code>--quiet</code></a>, <code>-q</code></dt><dd><p>Use quiet output.</p>
<p>Repeating this option, e.g., <code>-qq</code>, will enable a silent mode in which prek will write no output to stdout.</p>
</dd><dt id="prek-cache-clean--refresh"><a href="#prek-cache-clean--refresh"><code>--refresh</code></a></dt><dd><p>Refresh all cached data</p>
</dd><dt id="prek-cache-clean--verbose"><a href="#prek-cache-clean--verbose"><code>--verbose</code></a>, <code>-v</code></dt><dd><p>Use verbose output</p>
</dd><dt id="prek-cache-clean--version"><a href="#prek-cache-clean--version"><code>--version</code></a>, <code>-V</code></dt><dd><p>Display the prek version</p>
</dd></dl>

### prek cache size

Show the size of the prek cache

<h3 class="cli-reference">Usage</h3>

```
prek cache size [OPTIONS]
```

<h3 class="cli-reference">Options</h3>

<dl class="cli-reference"><dt id="prek-cache-size--cd"><a href="#prek-cache-size--cd"><code>--cd</code></a>, <code>-C</code> <i>dir</i></dt><dd><p>Change to directory before running</p>
</dd><dt id="prek-cache-size--color"><a href="#prek-cache-size--color"><code>--color</code></a> <i>color</i></dt><dd><p>Whether to use color in output</p>
<p>May also be set with the <code>PREK_COLOR</code> environment variable.</p><p>[default: auto]</p><p>Possible values:</p>
<ul>
<li><code>auto</code>:  Enables colored output only when the output is going to a terminal or TTY with support</li>
<li><code>always</code>:  Enables colored output regardless of the detected environment</li>
<li><code>never</code>:  Disables colored output</li>
</ul></dd><dt id="prek-cache-size--config"><a href="#prek-cache-size--config"><code>--config</code></a>, <code>-c</code> <i>config</i></dt><dd><p>Path to alternate config file</p>
</dd><dt id="prek-cache-size--help"><a href="#prek-cache-size--help"><code>--help</code></a>, <code>-h</code></dt><dd><p>Display the concise help for this command</p>
</dd><dt id="prek-cache-size--human"><a href="#prek-cache-size--human"><code>--human</code></a>, <code>--human-readable</code>, <code>-H</code></dt><dd><p>Display the cache size in human-readable format (e.g., <code>1.2 GiB</code> instead of raw bytes)</p>
</dd><dt id="prek-cache-size--log-file"><a href="#prek-cache-size--log-file"><code>--log-file</code></a> <i>log-file</i></dt><dd><p>Write trace logs to the specified file. If not specified, trace logs will be written to <code>$PREK_HOME/prek.log</code></p>
</dd><dt id="prek-cache-size--no-progress"><a href="#prek-cache-size--no-progress"><code>--no-progress</code></a></dt><dd><p>Hide all progress outputs.</p>
<p>For example, spinners or progress bars.</p>
</dd><dt id="prek-cache-size--quiet"><a href="#prek-cache-size--quiet"><code>--quiet</code></a>, <code>-q</code></dt><dd><p>Use quiet output.</p>
<p>Repeating this option, e.g., <code>-qq</code>, will enable a silent mode in which prek will write no output to stdout.</p>
</dd><dt id="prek-cache-size--refresh"><a href="#prek-cache-size--refresh"><code>--refresh</code></a></dt><dd><p>Refresh all cached data</p>
</dd><dt id="prek-cache-size--verbose"><a href="#prek-cache-size--verbose"><code>--verbose</code></a>, <code>-v</code></dt><dd><p>Use verbose output</p>
</dd><dt id="prek-cache-size--version"><a href="#prek-cache-size--version"><code>--version</code></a>, <code>-V</code></dt><dd><p>Display the prek version</p>
</dd></dl>

## prek init-template-dir

Install hook script in a directory intended for use with `git config init.templateDir`

<h3 class="cli-reference">Usage</h3>

```
prek init-template-dir [OPTIONS] <DIRECTORY>
```

<h3 class="cli-reference">Arguments</h3>

<dl class="cli-reference"><dt id="prek-init-template-dir--directory"><a href="#prek-init-template-dir--directory"<code>DIRECTORY</code></a></dt><dd><p>The directory in which to write the hook script</p>
</dd></dl>

<h3 class="cli-reference">Options</h3>

<dl class="cli-reference"><dt id="prek-init-template-dir--cd"><a href="#prek-init-template-dir--cd"><code>--cd</code></a>, <code>-C</code> <i>dir</i></dt><dd><p>Change to directory before running</p>
</dd><dt id="prek-init-template-dir--color"><a href="#prek-init-template-dir--color"><code>--color</code></a> <i>color</i></dt><dd><p>Whether to use color in output</p>
<p>May also be set with the <code>PREK_COLOR</code> environment variable.</p><p>[default: auto]</p><p>Possible values:</p>
<ul>
<li><code>auto</code>:  Enables colored output only when the output is going to a terminal or TTY with support</li>
<li><code>always</code>:  Enables colored output regardless of the detected environment</li>
<li><code>never</code>:  Disables colored output</li>
</ul></dd><dt id="prek-init-template-dir--config"><a href="#prek-init-template-dir--config"><code>--config</code></a>, <code>-c</code> <i>config</i></dt><dd><p>Path to alternate config file</p>
</dd><dt id="prek-init-template-dir--help"><a href="#prek-init-template-dir--help"><code>--help</code></a>, <code>-h</code></dt><dd><p>Display the concise help for this command</p>
</dd><dt id="prek-init-template-dir--hook-type"><a href="#prek-init-template-dir--hook-type"><code>--hook-type</code></a>, <code>-t</code> <i>hook-type</i></dt><dd><p>Which hook type to install</p>
<p>Possible values:</p>
<ul>
<li><code>commit-msg</code></li>
<li><code>post-checkout</code></li>
<li><code>post-commit</code></li>
<li><code>post-merge</code></li>
<li><code>post-rewrite</code></li>
<li><code>pre-commit</code></li>
<li><code>pre-merge-commit</code></li>
<li><code>pre-push</code></li>
<li><code>pre-rebase</code></li>
<li><code>prepare-commit-msg</code></li>
</ul></dd><dt id="prek-init-template-dir--log-file"><a href="#prek-init-template-dir--log-file"><code>--log-file</code></a> <i>log-file</i></dt><dd><p>Write trace logs to the specified file. If not specified, trace logs will be written to <code>$PREK_HOME/prek.log</code></p>
</dd><dt id="prek-init-template-dir--no-allow-missing-config"><a href="#prek-init-template-dir--no-allow-missing-config"><code>--no-allow-missing-config</code></a></dt><dd><p>Assume cloned repos should have a <code>pre-commit</code> config</p>
</dd><dt id="prek-init-template-dir--no-progress"><a href="#prek-init-template-dir--no-progress"><code>--no-progress</code></a></dt><dd><p>Hide all progress outputs.</p>
<p>For example, spinners or progress bars.</p>
</dd><dt id="prek-init-template-dir--quiet"><a href="#prek-init-template-dir--quiet"><code>--quiet</code></a>, <code>-q</code></dt><dd><p>Use quiet output.</p>
<p>Repeating this option, e.g., <code>-qq</code>, will enable a silent mode in which prek will write no output to stdout.</p>
</dd><dt id="prek-init-template-dir--refresh"><a href="#prek-init-template-dir--refresh"><code>--refresh</code></a></dt><dd><p>Refresh all cached data</p>
</dd><dt id="prek-init-template-dir--verbose"><a href="#prek-init-template-dir--verbose"><code>--verbose</code></a>, <code>-v</code></dt><dd><p>Use verbose output</p>
</dd><dt id="prek-init-template-dir--version"><a href="#prek-init-template-dir--version"><code>--version</code></a>, <code>-V</code></dt><dd><p>Display the prek version</p>
</dd></dl>

## prek try-repo

Try the pre-commit hooks in the current repo

<h3 class="cli-reference">Usage</h3>

```
prek try-repo [OPTIONS] <REPO> [HOOK|PROJECT]...
```

<h3 class="cli-reference">Arguments</h3>

<dl class="cli-reference"><dt id="prek-try-repo--repo"><a href="#prek-try-repo--repo"<code>REPO</code></a></dt><dd><p>Repository to source hooks from</p>
</dd><dt id="prek-try-repo--includes"><a href="#prek-try-repo--includes"<code>HOOK|PROJECT</code></a></dt><dd><p>Include the specified hooks or projects.</p>
<p>Supports flexible selector syntax:</p>
<ul>
<li>
<p><code>hook-id</code>: Run all hooks with the specified ID across all projects</p>
</li>
<li>
<p><code>project-path/</code>: Run all hooks from the specified project</p>
</li>
<li>
<p><code>project-path:hook-id</code>: Run only the specified hook from the specified project</p>
</li>
</ul>
<p>Can be specified multiple times to select multiple hooks/projects.</p>
</dd></dl>

<h3 class="cli-reference">Options</h3>

<dl class="cli-reference"><dt id="prek-try-repo--all-files"><a href="#prek-try-repo--all-files"><code>--all-files</code></a>, <code>-a</code></dt><dd><p>Run on all files in the repo</p>
</dd><dt id="prek-try-repo--cd"><a href="#prek-try-repo--cd"><code>--cd</code></a>, <code>-C</code> <i>dir</i></dt><dd><p>Change to directory before running</p>
</dd><dt id="prek-try-repo--color"><a href="#prek-try-repo--color"><code>--color</code></a> <i>color</i></dt><dd><p>Whether to use color in output</p>
<p>May also be set with the <code>PREK_COLOR</code> environment variable.</p><p>[default: auto]</p><p>Possible values:</p>
<ul>
<li><code>auto</code>:  Enables colored output only when the output is going to a terminal or TTY with support</li>
<li><code>always</code>:  Enables colored output regardless of the detected environment</li>
<li><code>never</code>:  Disables colored output</li>
</ul></dd><dt id="prek-try-repo--config"><a href="#prek-try-repo--config"><code>--config</code></a>, <code>-c</code> <i>config</i></dt><dd><p>Path to alternate config file</p>
</dd><dt id="prek-try-repo--directory"><a href="#prek-try-repo--directory"><code>--directory</code></a>, <code>-d</code> <i>dir</i></dt><dd><p>Run hooks on all files in the specified directories.</p>
<p>You can specify multiple directories. It can be used in conjunction with <code>--files</code>.</p>
</dd><dt id="prek-try-repo--dry-run"><a href="#prek-try-repo--dry-run"><code>--dry-run</code></a></dt><dd><p>Do not run the hooks, but print the hooks that would have been run</p>
</dd><dt id="prek-try-repo--fail-fast"><a href="#prek-try-repo--fail-fast"><code>--fail-fast</code></a></dt><dd><p>Stop running hooks after the first failure</p>
</dd><dt id="prek-try-repo--files"><a href="#prek-try-repo--files"><code>--files</code></a> <i>files</i></dt><dd><p>Specific filenames to run hooks on</p>
</dd><dt id="prek-try-repo--from-ref"><a href="#prek-try-repo--from-ref"><code>--from-ref</code></a>, <code>--source</code>, <code>-s</code> <i>from-ref</i></dt><dd><p>The original ref in a <code>&lt;from_ref&gt;...&lt;to_ref&gt;</code> diff expression. Files changed in this diff will be run through the hooks</p>
</dd><dt id="prek-try-repo--help"><a href="#prek-try-repo--help"><code>--help</code></a>, <code>-h</code></dt><dd><p>Display the concise help for this command</p>
</dd><dt id="prek-try-repo--hook-stage"><a href="#prek-try-repo--hook-stage"><code>--hook-stage</code></a> <i>hook-stage</i></dt><dd><p>The stage during which the hook is fired.</p>
<p>When specified, only hooks configured for that stage (for example <code>manual</code>, <code>pre-commit</code>, or <code>pre-commit</code>) will run. Defaults to <code>pre-commit</code> if not specified. For hooks specified directly in the command line, fallback to <code>manual</code> stage if no hooks found for <code>pre-commit</code> stage.</p>
<p>Possible values:</p>
<ul>
<li><code>manual</code></li>
<li><code>commit-msg</code></li>
<li><code>post-checkout</code></li>
<li><code>post-commit</code></li>
<li><code>post-merge</code></li>
<li><code>post-rewrite</code></li>
<li><code>pre-commit</code></li>
<li><code>pre-merge-commit</code></li>
<li><code>pre-push</code></li>
<li><code>pre-rebase</code></li>
<li><code>prepare-commit-msg</code></li>
</ul></dd><dt id="prek-try-repo--last-commit"><a href="#prek-try-repo--last-commit"><code>--last-commit</code></a></dt><dd><p>Run hooks against the last commit. Equivalent to <code>--from-ref HEAD~1 --to-ref HEAD</code></p>
</dd><dt id="prek-try-repo--log-file"><a href="#prek-try-repo--log-file"><code>--log-file</code></a> <i>log-file</i></dt><dd><p>Write trace logs to the specified file. If not specified, trace logs will be written to <code>$PREK_HOME/prek.log</code></p>
</dd><dt id="prek-try-repo--no-progress"><a href="#prek-try-repo--no-progress"><code>--no-progress</code></a></dt><dd><p>Hide all progress outputs.</p>
<p>For example, spinners or progress bars.</p>
</dd><dt id="prek-try-repo--quiet"><a href="#prek-try-repo--quiet"><code>--quiet</code></a>, <code>-q</code></dt><dd><p>Use quiet output.</p>
<p>Repeating this option, e.g., <code>-qq</code>, will enable a silent mode in which prek will write no output to stdout.</p>
</dd><dt id="prek-try-repo--refresh"><a href="#prek-try-repo--refresh"><code>--refresh</code></a></dt><dd><p>Refresh all cached data</p>
</dd><dt id="prek-try-repo--rev"><a href="#prek-try-repo--rev"><code>--rev</code></a>, <code>--ref</code> <i>rev</i></dt><dd><p>Manually select a rev to run against, otherwise the <code>HEAD</code> revision will be used</p>
</dd><dt id="prek-try-repo--show-diff-on-failure"><a href="#prek-try-repo--show-diff-on-failure"><code>--show-diff-on-failure</code></a></dt><dd><p>When hooks fail, run <code>git diff</code> directly afterward</p>
</dd><dt id="prek-try-repo--skip"><a href="#prek-try-repo--skip"><code>--skip</code></a> <i>hook|project</i></dt><dd><p>Skip the specified hooks or projects.</p>
<p>Supports flexible selector syntax:</p>
<ul>
<li>
<p><code>hook-id</code>: Skip all hooks with the specified ID across all projects</p>
</li>
<li>
<p><code>project-path/</code>: Skip all hooks from the specified project</p>
</li>
<li>
<p><code>project-path:hook-id</code>: Skip only the specified hook from the specified project</p>
</li>
</ul>
<p>Can be specified multiple times. Also accepts <code>PREK_SKIP</code> or <code>SKIP</code> environment variables (comma-delimited).</p>
</dd><dt id="prek-try-repo--to-ref"><a href="#prek-try-repo--to-ref"><code>--to-ref</code></a>, <code>--origin</code>, <code>-o</code> <i>to-ref</i></dt><dd><p>The destination ref in a <code>from_ref...to_ref</code> diff expression. Defaults to <code>HEAD</code> if <code>from_ref</code> is specified</p>
</dd><dt id="prek-try-repo--verbose"><a href="#prek-try-repo--verbose"><code>--verbose</code></a>, <code>-v</code></dt><dd><p>Use verbose output</p>
</dd><dt id="prek-try-repo--version"><a href="#prek-try-repo--version"><code>--version</code></a>, <code>-V</code></dt><dd><p>Display the prek version</p>
</dd></dl>

## prek self

`prek` self management

<h3 class="cli-reference">Usage</h3>

```
prek self [OPTIONS] <COMMAND>
```

<h3 class="cli-reference">Commands</h3>

<dl class="cli-reference"><dt><a href="#prek-self-update"><code>prek self update</code></a></dt><dd><p>Update prek</p></dd>
</dl>

### prek self update

Update prek

<h3 class="cli-reference">Usage</h3>

```
prek self update [OPTIONS] [TARGET_VERSION]
```

<h3 class="cli-reference">Arguments</h3>

<dl class="cli-reference"><dt id="prek-self-update--target_version"><a href="#prek-self-update--target_version"<code>TARGET_VERSION</code></a></dt><dd><p>Update to the specified version. If not provided, prek will update to the latest version</p>
</dd></dl>

<h3 class="cli-reference">Options</h3>

<dl class="cli-reference"><dt id="prek-self-update--cd"><a href="#prek-self-update--cd"><code>--cd</code></a>, <code>-C</code> <i>dir</i></dt><dd><p>Change to directory before running</p>
</dd><dt id="prek-self-update--color"><a href="#prek-self-update--color"><code>--color</code></a> <i>color</i></dt><dd><p>Whether to use color in output</p>
<p>May also be set with the <code>PREK_COLOR</code> environment variable.</p><p>[default: auto]</p><p>Possible values:</p>
<ul>
<li><code>auto</code>:  Enables colored output only when the output is going to a terminal or TTY with support</li>
<li><code>always</code>:  Enables colored output regardless of the detected environment</li>
<li><code>never</code>:  Disables colored output</li>
</ul></dd><dt id="prek-self-update--config"><a href="#prek-self-update--config"><code>--config</code></a>, <code>-c</code> <i>config</i></dt><dd><p>Path to alternate config file</p>
</dd><dt id="prek-self-update--help"><a href="#prek-self-update--help"><code>--help</code></a>, <code>-h</code></dt><dd><p>Display the concise help for this command</p>
</dd><dt id="prek-self-update--log-file"><a href="#prek-self-update--log-file"><code>--log-file</code></a> <i>log-file</i></dt><dd><p>Write trace logs to the specified file. If not specified, trace logs will be written to <code>$PREK_HOME/prek.log</code></p>
</dd><dt id="prek-self-update--no-progress"><a href="#prek-self-update--no-progress"><code>--no-progress</code></a></dt><dd><p>Hide all progress outputs.</p>
<p>For example, spinners or progress bars.</p>
</dd><dt id="prek-self-update--quiet"><a href="#prek-self-update--quiet"><code>--quiet</code></a>, <code>-q</code></dt><dd><p>Use quiet output.</p>
<p>Repeating this option, e.g., <code>-qq</code>, will enable a silent mode in which prek will write no output to stdout.</p>
</dd><dt id="prek-self-update--refresh"><a href="#prek-self-update--refresh"><code>--refresh</code></a></dt><dd><p>Refresh all cached data</p>
</dd><dt id="prek-self-update--token"><a href="#prek-self-update--token"><code>--token</code></a> <i>token</i></dt><dd><p>A GitHub token for authentication. A token is not required but can be used to reduce the chance of encountering rate limits</p>
<p>May also be set with the <code>GITHUB_TOKEN</code> environment variable.</p></dd><dt id="prek-self-update--verbose"><a href="#prek-self-update--verbose"><code>--verbose</code></a>, <code>-v</code></dt><dd><p>Use verbose output</p>
</dd><dt id="prek-self-update--version"><a href="#prek-self-update--version"><code>--version</code></a>, <code>-V</code></dt><dd><p>Display the prek version</p>
</dd></dl>

