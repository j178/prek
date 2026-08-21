# This creates a 10GB dev drive, and exports all required environment
# variables so that rustup, prek and others all use the dev drive as much
# as possible.
# $Volume = New-VHD -Path C:/prek_dev_drive.vhdx -SizeBytes 10GB |
# 					Mount-VHD -Passthru |
# 					Initialize-Disk -Passthru |
# 					New-Partition -AssignDriveLetter -UseMaximumSize |
# 					Format-Volume -FileSystem ReFS -Confirm:$false -Force
#
# Write-Output $Volume

$Drive = "D:"
$Tmp = "$($Drive)\prek-tmp"

# Create the directory ahead of time in an attempt to avoid race-conditions
New-Item $Tmp -ItemType Directory

# Move Cargo to the dev drive
New-Item -Path "$($Drive)/.cargo/bin" -ItemType Directory -Force
if (Test-Path "C:/Users/runneradmin/.cargo") {
    Copy-Item -Path "C:/Users/runneradmin/.cargo/*" -Destination "$($Drive)/.cargo/" -Recurse -Force
}

Write-Output `
	"DEV_DRIVE=$($Drive)" `
	"TMP=$($Tmp)" `
	"TEMP=$($Tmp)" `
	"PREK_INTERNAL__TEST_DIR=$($Tmp)" `
	"RUSTUP_HOME=$($Drive)/.rustup" `
	"CARGO_HOME=$($Drive)/.cargo" `
	"PREK_WORKSPACE=$($Drive)/prek" `
    "PATH=$($Drive)/.cargo/bin;$env:PATH" `
	>> $env:GITHUB_ENV
