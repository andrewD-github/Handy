$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$root = Join-Path ([System.IO.Path]::GetTempPath()) "handy-legacy-rollback-$([guid]::NewGuid())"
$backup = Join-Path $root 'backup'
$install = Join-Path $root 'install'
try {
    New-Item -ItemType Directory -Path (Join-Path $backup 'runtime') -Force | Out-Null
    New-Item -ItemType Directory -Path (Join-Path $backup 'resources') -Force | Out-Null
    New-Item -ItemType Directory -Path $install -Force | Out-Null
    Set-Content -LiteralPath (Join-Path $backup 'runtime\handy.exe') -Value 'legacy runtime'
    Set-Content -LiteralPath (Join-Path $backup 'resources\tray_idle.png') -Value 'legacy resource'
    Set-Content -LiteralPath (Join-Path $backup 'settings_store.json') -Value '{"model":"legacy"}'

    # Exact legacy shape: no schema version or stored original hashes.
    [ordered]@{
        created_at = '2026-08-15T00:00:00Z'
        install_root = $install
        installed_exe_sha256 = 'legacy'
        candidate_exe_sha256 = 'candidate'
        files = @([ordered]@{
            relative_path = 'handy.exe'
            existed = $true
            backup_path = 'runtime\handy.exe'
        })
        resources_existed = $true
        model_relative_path = 'Data\models\nemotron.gguf'
        model_existed = $false
    } | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $backup 'manifest.json') -Encoding UTF8

    $installer = Join-Path $PSScriptRoot 'install-portable-progressive.ps1'
    $installerSource = Get-Content -Raw -LiteralPath $installer
    if ($installerSource -notmatch [regex]::Escape("[string]`$CandidateRoot = 'D:\Apps\Handy-Candidate-0.9.6-progressive'")) {
        throw 'Installer default candidate path is not pinned to v0.9.6.'
    }
    $output = & powershell -NoProfile -ExecutionPolicy Bypass -File $installer -ValidateRollbackFrom (Join-Path $backup 'manifest.json')
    if ($LASTEXITCODE -ne 0 -or $output -notmatch 'complete and verifiable') {
        throw 'Legacy rollback manifest validation failed.'
    }
    Write-Output 'Legacy rollback manifest validation passed.'
} finally {
    if (Test-Path -LiteralPath $root) {
        Remove-Item -LiteralPath $root -Recurse -Force
    }
}
