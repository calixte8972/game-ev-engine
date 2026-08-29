param(
    [switch]$Development
)

$projectRoot = Split-Path -Parent $PSScriptRoot
Push-Location $projectRoot

try {
    $arguments = @("build", "--target", "web", "--out-dir", "web/pkg")
    if (-not $Development) {
        $arguments += "--release"
    }

    & wasm-pack @arguments
    if ($LASTEXITCODE -ne 0) {
        exit $LASTEXITCODE
    }
}
finally {
    Pop-Location
}
