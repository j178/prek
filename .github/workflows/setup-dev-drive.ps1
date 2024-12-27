# This creates a 10GB dev drive, and exports all required environment
# variables so that rustup, prefligit and others all use the dev drive as much
# as possible.
# $Volume = New-VHD -Path C:/prefligit_dev_drive.vhdx -SizeBytes 10GB |
# 					Mount-VHD -Passthru |
# 					Initialize-Disk -Passthru |
# 					New-Partition -AssignDriveLetter -UseMaximumSize |
# 					Format-Volume -FileSystem ReFS -Confirm:$false -Force
#
# Write-Output $Volume

$Drive = "D:"
$Tmp = "$($Drive)\prefligit-tmp"

# Create the directory ahead of time in an attempt to avoid race-conditions
New-Item $Tmp -ItemType Directory

Write-Output `
	"DEV_DRIVE=$($Drive)" `
	"TMP=$($Tmp)" `
	"TEMP=$($Tmp)" `
	"PREFLIGIT_INTERNAL__TEST_DIR=$($Tmp)" `
	"RUSTUP_HOME=$($Drive)/.rustup" `
	"CARGO_HOME=$($Drive)/.cargo" `
	>> $env:GITHUB_ENV
