[CmdletBinding(DefaultParameterSetName = 'Install')]
param(
    [Parameter(ParameterSetName = 'Install')]
    [string]$CandidateRoot = 'D:\Apps\Handy-Candidate-0.9.5-progressive',

    [Parameter(ParameterSetName = 'Install')]
    [string]$InstallRoot = 'D:\Apps\Handy',

    [Parameter(ParameterSetName = 'Install')]
    [string]$ExpectedExeSha256 = 'BF40094D19602074F05F6F62856E68819CFEDE8D21B30D4A012CE6A7F5171A03',

    [Parameter(ParameterSetName = 'Install')]
    [switch]$Install,

    [Parameter(Mandatory, ParameterSetName = 'Rollback')]
    [string]$RollbackFrom
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$ModelFileName = 'nemotron-3.5-asr-streaming-0.6b-Q8_0.gguf'
$ModelId = 'handy-computer/nemotron-3.5-asr-streaming-0.6b-gguf/nemotron-3.5-asr-streaming-0.6b-Q8_0.gguf'
$ModelSha256 = 'B94545B313B3223FDA7B2857A52681DA813935C2127643D1E9FF0C23D988089C'
$RuntimeFiles = @(
    'handy.exe',
    'DirectML.dll',
    'ggml-base.dll',
    'ggml-cpu-alderlake.dll',
    'ggml-cpu-cannonlake.dll',
    'ggml-cpu-cascadelake.dll',
    'ggml-cpu-haswell.dll',
    'ggml-cpu-icelake.dll',
    'ggml-cpu-sandybridge.dll',
    'ggml-cpu-skylakex.dll',
    'ggml-cpu-sse42.dll',
    'ggml-cpu-x64.dll',
    'ggml-vulkan.dll',
    'ggml.dll',
    'transcribe.dll'
)

function Assert-UnderRoot([string]$Path, [string]$Root) {
    $fullPath = [System.IO.Path]::GetFullPath($Path)
    $fullRoot = [System.IO.Path]::GetFullPath($Root).TrimEnd('\') + '\'
    if (-not $fullPath.StartsWith($fullRoot, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing operation outside expected root: $fullPath"
    }
}

function Get-Sha256([string]$Path) {
    (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToUpperInvariant()
}

function Set-JsonProperty([object]$Object, [string]$Name, [object]$Value) {
    if ($Object.PSObject.Properties.Name -contains $Name) {
        $Object.$Name = $Value
    } else {
        $Object | Add-Member -NotePropertyName $Name -NotePropertyValue $Value
    }
}

function Stop-HandyAt([string]$ExecutablePath) {
    $fullPath = [System.IO.Path]::GetFullPath($ExecutablePath)
    $processes = Get-CimInstance Win32_Process |
        Where-Object { $_.ExecutablePath -and [System.IO.Path]::GetFullPath($_.ExecutablePath) -eq $fullPath }
    foreach ($process in $processes) {
        Stop-Process -Id $process.ProcessId -Force
        Wait-Process -Id $process.ProcessId -ErrorAction SilentlyContinue
    }
}

function Restore-Install([string]$ManifestPath) {
    $manifestPath = (Resolve-Path -LiteralPath $ManifestPath).Path
    $backupRoot = Split-Path -Parent $manifestPath
    $manifest = Get-Content -Raw -LiteralPath $manifestPath | ConvertFrom-Json
    $targetRoot = [System.IO.Path]::GetFullPath([string]$manifest.install_root)
    Assert-UnderRoot -Path (Join-Path $targetRoot 'handy.exe') -Root $targetRoot
    Stop-HandyAt (Join-Path $targetRoot 'handy.exe')

    foreach ($entry in $manifest.files) {
        $target = Join-Path $targetRoot ([string]$entry.relative_path)
        Assert-UnderRoot -Path $target -Root $targetRoot
        if ([bool]$entry.existed) {
            Copy-Item -LiteralPath (Join-Path $backupRoot ([string]$entry.backup_path)) -Destination $target -Force
        } elseif (Test-Path -LiteralPath $target) {
            Remove-Item -LiteralPath $target -Force
        }
    }

    $resources = Join-Path $targetRoot 'resources'
    Assert-UnderRoot -Path $resources -Root $targetRoot
    if (Test-Path -LiteralPath $resources) {
        Remove-Item -LiteralPath $resources -Recurse -Force
    }
    if ([bool]$manifest.resources_existed) {
        Copy-Item -LiteralPath (Join-Path $backupRoot 'resources') -Destination $resources -Recurse
    }

    $modelTarget = Join-Path $targetRoot ([string]$manifest.model_relative_path)
    Assert-UnderRoot -Path $modelTarget -Root $targetRoot
    if ([bool]$manifest.model_existed) {
        Copy-Item -LiteralPath (Join-Path $backupRoot 'model' $ModelFileName) -Destination $modelTarget -Force
    } elseif (Test-Path -LiteralPath $modelTarget) {
        Remove-Item -LiteralPath $modelTarget -Force
    }

    Copy-Item -LiteralPath (Join-Path $backupRoot 'settings_store.json') -Destination (Join-Path $targetRoot 'Data\settings_store.json') -Force
    Start-Process -FilePath (Join-Path $targetRoot 'handy.exe')
    Write-Output "Rollback complete: $backupRoot"
}

if ($PSCmdlet.ParameterSetName -eq 'Rollback') {
    Restore-Install -ManifestPath $RollbackFrom
    exit 0
}

if (-not $Install) {
    throw 'Installation is explicit: rerun with -Install after reviewing the candidate and paths.'
}

$CandidateRoot = (Resolve-Path -LiteralPath $CandidateRoot).Path
$InstallRoot = (Resolve-Path -LiteralPath $InstallRoot).Path
$candidateExe = Join-Path $CandidateRoot 'handy.exe'
$installedExe = Join-Path $InstallRoot 'handy.exe'
$candidateResources = Join-Path $CandidateRoot 'resources'
$candidateModel = Join-Path $CandidateRoot "Data\models\$ModelFileName"
$installedModel = Join-Path $InstallRoot "Data\models\$ModelFileName"
$settingsPath = Join-Path $InstallRoot 'Data\settings_store.json'

foreach ($required in @($candidateExe, $candidateResources, (Join-Path $candidateResources 'tray_idle.png'), $candidateModel, $installedExe, $settingsPath)) {
    if (-not (Test-Path -LiteralPath $required)) {
        throw "Required input is missing: $required"
    }
}
if ((Get-Sha256 $candidateExe) -ne $ExpectedExeSha256.ToUpperInvariant()) {
    throw 'Candidate executable hash does not match the reviewed build.'
}
if ((Get-Sha256 $candidateModel) -ne $ModelSha256) {
    throw 'Nemotron model hash does not match Handy catalog.'
}

$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$backupRoot = Join-Path (Split-Path -Parent $InstallRoot) "Handy-backups\progressive-$stamp"
New-Item -ItemType Directory -Path $backupRoot -Force | Out-Null
Copy-Item -LiteralPath $settingsPath -Destination (Join-Path $backupRoot 'settings_store.json')

$fileManifest = @()
foreach ($name in $RuntimeFiles) {
    $source = Join-Path $CandidateRoot $name
    if (-not (Test-Path -LiteralPath $source)) {
        throw "Candidate runtime file is missing: $source"
    }
    $target = Join-Path $InstallRoot $name
    Assert-UnderRoot -Path $target -Root $InstallRoot
    $existed = Test-Path -LiteralPath $target
    $backupPath = "runtime\$name"
    if ($existed) {
        New-Item -ItemType Directory -Path (Join-Path $backupRoot 'runtime') -Force | Out-Null
        Copy-Item -LiteralPath $target -Destination (Join-Path $backupRoot $backupPath)
    }
    $fileManifest += [ordered]@{ relative_path = $name; existed = $existed; backup_path = $backupPath }
}

$resourcesTarget = Join-Path $InstallRoot 'resources'
$resourcesExisted = Test-Path -LiteralPath $resourcesTarget
if ($resourcesExisted) {
    Copy-Item -LiteralPath $resourcesTarget -Destination (Join-Path $backupRoot 'resources') -Recurse
}
$modelExisted = Test-Path -LiteralPath $installedModel
if ($modelExisted) {
    New-Item -ItemType Directory -Path (Join-Path $backupRoot 'model') -Force | Out-Null
    Copy-Item -LiteralPath $installedModel -Destination (Join-Path $backupRoot 'model' $ModelFileName)
}

$manifest = [ordered]@{
    created_at = (Get-Date).ToString('o')
    install_root = $InstallRoot
    installed_exe_sha256 = Get-Sha256 $installedExe
    candidate_exe_sha256 = Get-Sha256 $candidateExe
    files = $fileManifest
    resources_existed = $resourcesExisted
    model_relative_path = "Data\models\$ModelFileName"
    model_existed = $modelExisted
}
$manifest | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $backupRoot 'manifest.json') -Encoding UTF8

Stop-HandyAt $installedExe
foreach ($name in $RuntimeFiles) {
    Copy-Item -LiteralPath (Join-Path $CandidateRoot $name) -Destination (Join-Path $InstallRoot $name) -Force
}
if (Test-Path -LiteralPath $resourcesTarget) {
    Assert-UnderRoot -Path $resourcesTarget -Root $InstallRoot
    Remove-Item -LiteralPath $resourcesTarget -Recurse -Force
}
Copy-Item -LiteralPath $candidateResources -Destination $resourcesTarget -Recurse
Copy-Item -LiteralPath $candidateModel -Destination $installedModel -Force

$settingsDocument = Get-Content -Raw -LiteralPath $settingsPath | ConvertFrom-Json
$settings = if ($settingsDocument.settings) { $settingsDocument.settings } else { $settingsDocument }
Set-JsonProperty -Object $settings -Name 'selected_model' -Value $ModelId
Set-JsonProperty -Object $settings -Name 'progressive_output_mode' -Value 'direct_prompt'
Set-JsonProperty -Object $settings -Name 'onboarding_completed' -Value $true
Set-JsonProperty -Object $settings -Name 'diagnostic_capture_enabled' -Value $true
$settingsDocument | ConvertTo-Json -Depth 100 | Set-Content -LiteralPath $settingsPath -Encoding UTF8

if ((Get-Sha256 $installedExe) -ne $ExpectedExeSha256.ToUpperInvariant()) {
    throw 'Installed executable hash verification failed.'
}
if ((Get-Sha256 $installedModel) -ne $ModelSha256) {
    throw 'Installed model hash verification failed.'
}
if (-not (Test-Path -LiteralPath (Join-Path $resourcesTarget 'tray_idle.png'))) {
    throw 'Installed resource verification failed.'
}

Start-Process -FilePath $installedExe
Start-Sleep -Seconds 8
$running = Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath -eq $installedExe }
if (-not $running) {
    throw "Installed Handy did not remain running. Roll back with: $PSCommandPath -RollbackFrom '$(Join-Path $backupRoot 'manifest.json')'"
}

Write-Output "Installed and running. Backup: $backupRoot"
Write-Output "Rollback: $PSCommandPath -RollbackFrom '$(Join-Path $backupRoot 'manifest.json')'"
