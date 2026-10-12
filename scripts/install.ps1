# Installs the OrkStage CLI (`ork`) on Windows from a GitHub release.
#
#   irm https://orkstage.io/install.ps1 | iex
#
# Environment:
#   ORK_VERSION        version to install, for example 0.2.0 (default: the latest release)
#   ORK_INSTALL_DIR    where to put ork.exe (default: %LOCALAPPDATA%\Programs\ork)
#   ORK_DOWNLOAD_BASE  where the release files are (default: the GitHub release of ORK_VERSION);
#                      a mirror must serve the archive and SHA256SUMS side by side
#
# No administrator rights needed. The archive is checked against the release's SHA256SUMS, and the
# install directory is added to the user PATH.
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$Repo = 'orkstage/orkstage-cli'
$InstallDir = if ($env:ORK_INSTALL_DIR) { $env:ORK_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'Programs\ork' }

switch ($env:PROCESSOR_ARCHITECTURE) {
  'AMD64' { $arch = 'x86_64' }
  'ARM64' { $arch = 'aarch64' }
  default { throw "Unsupported architecture $($env:PROCESSOR_ARCHITECTURE)" }
}
$target = "$arch-pc-windows-msvc"

$version = $env:ORK_VERSION
if (-not $version) {
  $latest = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest" -Headers @{ 'User-Agent' = 'ork-installer' }
  $version = $latest.tag_name
}
$version = $version.TrimStart('v')
if (-not $version -or $version.Contains('/')) { throw 'Cannot tell the version to install (set ORK_VERSION)' }

$base = if ($env:ORK_DOWNLOAD_BASE) { $env:ORK_DOWNLOAD_BASE } else { "https://github.com/$Repo/releases/download/v$version" }
$archive = "ork-$version-$target.zip"

$tmp = Join-Path ([IO.Path]::GetTempPath()) ("ork-" + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
  Write-Host "Downloading ork $version for $target"
  Invoke-WebRequest -Uri "$base/$archive" -OutFile (Join-Path $tmp $archive) -UseBasicParsing
  Invoke-WebRequest -Uri "$base/SHA256SUMS" -OutFile (Join-Path $tmp 'SHA256SUMS') -UseBasicParsing

  $line = Get-Content (Join-Path $tmp 'SHA256SUMS') | Where-Object { ($_ -split '\s+')[1] -in @($archive, "*$archive") } | Select-Object -First 1
  if (-not $line) { throw "$archive is not listed in SHA256SUMS" }
  $expected = ($line -split '\s+')[0].ToLowerInvariant()
  $actual = (Get-FileHash -Algorithm SHA256 (Join-Path $tmp $archive)).Hash.ToLowerInvariant()
  if ($expected -ne $actual) { throw "Checksum mismatch for $archive (expected $expected, got $actual)" }

  Expand-Archive -Path (Join-Path $tmp $archive) -DestinationPath (Join-Path $tmp 'x') -Force
  $exe = Join-Path $tmp 'x\ork.exe'
  if (-not (Test-Path $exe)) { throw 'The archive does not contain ork.exe' }
  New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
  Copy-Item $exe (Join-Path $InstallDir 'ork.exe') -Force
}
finally {
  Remove-Item -Recurse -Force -LiteralPath $tmp -ErrorAction SilentlyContinue
}

$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if (-not (($userPath -split ';') -contains $InstallDir)) {
  [Environment]::SetEnvironmentVariable('Path', ($(if ($userPath) { "$userPath;" } else { '' }) + $InstallDir), 'User')
  Write-Host "Added $InstallDir to your user PATH; open a new terminal to use it."
}
$installed = & (Join-Path $InstallDir 'ork.exe') --version
Write-Host "Installed $installed to $InstallDir\ork.exe"
