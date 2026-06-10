param(
    [string]$Arch = "x64"
)

$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
if (-not (Test-Path $vswhere)) {
    throw "vswhere.exe not found at $vswhere"
}

$installationPath = & $vswhere -latest -products "*" -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath | Select-Object -First 1
if (-not $installationPath) {
    throw "Visual Studio with MSVC tools not found"
}

$vcvarsall = Join-Path $installationPath.Trim() "VC\Auxiliary\Build\vcvarsall.bat"
if (-not (Test-Path $vcvarsall)) {
    throw "vcvarsall.bat not found at $vcvarsall"
}

$before = @{}
Get-ChildItem Env: | ForEach-Object {
    $before[$_.Name.ToUpperInvariant()] = $_.Value
}

$after = cmd.exe /s /c "call `"$vcvarsall`" $Arch > nul && set"
if ($LASTEXITCODE -ne 0) {
    throw "vcvarsall.bat failed with exit code $LASTEXITCODE"
}

foreach ($line in $after) {
    $entry = $line -split "=", 2
    if ($entry.Length -ne 2) {
        continue
    }

    $name = $entry[0]
    $value = $entry[1]
    $key = $name.ToUpperInvariant()
    if ($before.ContainsKey($key) -and $before[$key] -eq $value) {
        continue
    }

    "$name=$value" | Out-File -FilePath $env:GITHUB_ENV -Append -Encoding utf8
}
