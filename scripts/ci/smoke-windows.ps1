param([Parameter(Mandatory = $true)][string]$ArtifactDirectory)

$ErrorActionPreference = 'Stop'

if ([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture -ne 'X64') {
  throw "Expected Windows x64, found $([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture)"
}
node scripts/ci/artifact-manifest.mjs verify $ArtifactDirectory
if ($LASTEXITCODE -ne 0) { throw 'Artifact manifest verification failed' }

$installers = @(Get-ChildItem -LiteralPath $ArtifactDirectory -Filter '*-setup.exe' -File)
if ($installers.Count -ne 1) { throw "Expected one NSIS installer, found $($installers.Count)" }
$signature = Get-AuthenticodeSignature -LiteralPath $installers[0].FullName
"Windows Authenticode classification: $($signature.Status)"
if ($signature.Status -notin @('Valid', 'NotSigned')) { throw "Unexpected Authenticode status: $($signature.Status)" }

$install = Start-Process -FilePath $installers[0].FullName -ArgumentList '/S' -Wait -PassThru
if ($install.ExitCode -ne 0) { throw "NSIS install failed with exit code $($install.ExitCode)" }

$uninstallKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\MMD'
if (-not (Test-Path -LiteralPath $uninstallKey)) { throw 'Tauri NSIS uninstall registry entry is missing' }
$installation = Get-ItemProperty -LiteralPath $uninstallKey
$installDirectory = ([string]$installation.InstallLocation).Trim('"')
$mainBinary = [string]$installation.MainBinaryName
if (-not $installDirectory -or -not $mainBinary) { throw 'Tauri NSIS install metadata is incomplete' }
$appPath = Join-Path $installDirectory $mainBinary
if (-not (Test-Path -LiteralPath $appPath -PathType Leaf)) { throw "Installed application is missing: $appPath" }

$app = Start-Process -FilePath $appPath -PassThru
Start-Sleep -Seconds 5
if ($app.HasExited) { throw "Installed MMD exited early with code $($app.ExitCode)" }
$app.Kill()
$app.WaitForExit()

$uninstaller = Join-Path $installDirectory 'uninstall.exe'
if (-not (Test-Path -LiteralPath $uninstaller -PathType Leaf)) { throw 'NSIS uninstaller is missing' }
$uninstall = Start-Process -FilePath $uninstaller -ArgumentList '/S' -Wait -PassThru
if ($uninstall.ExitCode -ne 0) { throw "NSIS uninstall failed with exit code $($uninstall.ExitCode)" }
if (Test-Path -LiteralPath $uninstallKey) { throw 'NSIS uninstall registry entry was not removed' }
