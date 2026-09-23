# Copies release binaries to %LOCALAPPDATA%\Xode\bin and adds it to the user PATH.
$ErrorActionPreference = "Stop"
$src = Join-Path $PSScriptRoot "..\target\release"
$bin = Join-Path $env:LOCALAPPDATA "Xode\bin"
New-Item -ItemType Directory -Force $bin | Out-Null
foreach ($f in "xode.exe", "xode-desktop.exe") {
    $p = Join-Path $src $f
    if (Test-Path $p) { Copy-Item $p $bin -Force }
}
$path = [Environment]::GetEnvironmentVariable("Path", "User")
if (-not ($path -split ";" | Where-Object { $_ -ieq $bin })) {
    [Environment]::SetEnvironmentVariable("Path", ($path.TrimEnd(";") + ";" + $bin), "User")
    Write-Output "added $bin to user PATH"
}
# Shims in WindowsApps (already on PATH in every existing terminal) so `xode` works without a restart.
$apps = Join-Path $env:LOCALAPPDATA "Microsoft\WindowsApps"
if (Test-Path $apps) {
    Set-Content -Encoding ascii (Join-Path $apps "xode.cmd") "@`"$bin\xode.exe`" %*"
    Set-Content -Encoding ascii (Join-Path $apps "xode-desktop.cmd") "@start `"`" `"$bin\xode-desktop.exe`" %*"
}
Get-ChildItem $bin | ForEach-Object { Write-Output "$($_.Name) $([math]::Round($_.Length/1MB,1))MB" }
