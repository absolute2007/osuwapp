param(
    [Parameter(Mandatory = $true, Position = 0)]
    [string]$Command
)

$candidateVsDevCmds = @(
    'C:\Program Files\Microsoft Visual Studio\2022\Community\Common7\Tools\VsDevCmd.bat',
    'C:\Program Files\Microsoft Visual Studio\18\Community\Common7\Tools\VsDevCmd.bat'
)

$vsDevCmd = $candidateVsDevCmds | Where-Object { Test-Path $_ } | Select-Object -First 1

if (-not $vsDevCmd) {
    Write-Error 'VsDevCmd.bat was not found. Install Visual Studio Build Tools or Visual Studio Community with C++ workload.'
    exit 1
}

$cmdInvocation = "`"$vsDevCmd`" -arch=x64 -host_arch=x64 >nul 2>nul && $Command"

cmd.exe /d /s /c $cmdInvocation
$exitCode = $LASTEXITCODE

if ($exitCode -ne 0) {
    exit $exitCode
}
