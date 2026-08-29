param(
    [int]$Port = 8000
)

if (-not (Test-Path -LiteralPath (Join-Path $PSScriptRoot "pkg/game_ev_engine.js"))) {
    & (Join-Path $PSScriptRoot "build.ps1")
    if ($LASTEXITCODE -ne 0) {
        exit $LASTEXITCODE
    }
}

python -m http.server $Port --directory $PSScriptRoot
