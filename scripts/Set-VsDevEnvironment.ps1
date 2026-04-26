$candidateVsDevCmds = @(
    'C:\Program Files\Microsoft Visual Studio\2022\Community\Common7\Tools\VsDevCmd.bat',
    'C:\Program Files\Microsoft Visual Studio\18\Community\Common7\Tools\VsDevCmd.bat'
)

$vsDevCmd = $candidateVsDevCmds | Where-Object { Test-Path $_ } | Select-Object -First 1

if (-not $vsDevCmd) {
    Write-Error 'VsDevCmd.bat was not found. Install Visual Studio Build Tools or Visual Studio Community with C++ workload.'
    exit 1
}

$envDump = cmd.exe /d /s /c "`"$vsDevCmd`" -arch=x64 -host_arch=x64 >nul 2>nul && set"

if ($LASTEXITCODE -ne 0) {
    Write-Error "Failed to initialize Visual Studio developer environment via $vsDevCmd"
    exit $LASTEXITCODE
}

foreach ($line in $envDump) {
    $separatorIndex = $line.IndexOf('=')

    if ($separatorIndex -lt 1) {
        continue
    }

    $name = $line.Substring(0, $separatorIndex)
    $value = $line.Substring($separatorIndex + 1)

    Set-Item -Path "Env:$name" -Value $value
}

Write-Host "Loaded Visual Studio developer environment from $vsDevCmd"
