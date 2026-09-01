[CmdletBinding(DefaultParameterSetName = 'Install')]
param(
    [Parameter(ParameterSetName = 'Install')]
    [string]$CandidateRoot = 'D:\Apps\Handy-Candidate-0.9.6-progressive',

    [Parameter(ParameterSetName = 'Install')]
    [string]$InstallRoot = 'D:\Apps\Handy',

    [Parameter(ParameterSetName = 'Install')]
    [string]$ExpectedExeSha256 = '7B735FE3A74BA1CB3A586942C97E54EC31D30A1A1139DB626DDD1F5D10CBD7C8',

    [Parameter(ParameterSetName = 'Install')]
    [switch]$Install,

    [Parameter(Mandatory, ParameterSetName = 'Rollback')]
    [string]$RollbackFrom,

    [Parameter(Mandatory, ParameterSetName = 'ValidateRollback')]
    [string]$ValidateRollbackFrom
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

function Get-TreeHashes([string]$Root) {
    $hashes = [ordered]@{}
    Get-ChildItem -LiteralPath $Root -File -Recurse | Sort-Object FullName | ForEach-Object {
        $relative = $_.FullName.Substring([System.IO.Path]::GetFullPath($Root).TrimEnd('\').Length + 1)
        $hashes[$relative] = Get-Sha256 $_.FullName
    }
    $hashes
}

function Assert-TreeHashes([string]$Root, [object]$Expected, [string]$Label) {
    $actual = Get-TreeHashes $Root
    $expectedEntries = if ($Expected -is [System.Collections.IDictionary]) {
        $Expected.GetEnumerator() | ForEach-Object { [pscustomobject]@{ Name = $_.Key; Value = $_.Value } }
    } else {
        $Expected.PSObject.Properties
    }
    foreach ($entry in $expectedEntries) {
        if (-not $actual.Contains([string]$entry.Name) -or $actual[[string]$entry.Name] -ne [string]$entry.Value) {
            throw "$Label hash verification failed: $($entry.Name)"
        }
    }
    if ($actual.Count -ne @($expectedEntries).Count) {
        throw "$Label file-count verification failed."
    }
}

function Set-JsonProperty([object]$Object, [string]$Name, [object]$Value) {
    if ($Object.PSObject.Properties.Name -contains $Name) {
        $Object.$Name = $Value
    } else {
        $Object | Add-Member -NotePropertyName $Name -NotePropertyValue $Value
    }
}

function Get-OptionalProperty([object]$Object, [string]$Name) {
    if ($Object.PSObject.Properties.Name -contains $Name) {
        $value = $Object.$Name
        if ($null -ne $value -and -not [string]::IsNullOrWhiteSpace([string]$value)) {
            return $value
        }
    }
    return $null
}

function Get-RollbackPlan([string]$ManifestPath) {
    $resolvedManifest = (Resolve-Path -LiteralPath $ManifestPath).Path
    $backupRoot = Split-Path -Parent $resolvedManifest
    $manifest = Get-Content -Raw -LiteralPath $resolvedManifest | ConvertFrom-Json
    $targetRoot = [System.IO.Path]::GetFullPath([string]$manifest.install_root)
    Assert-UnderRoot -Path (Join-Path $targetRoot 'handy.exe') -Root $targetRoot

    # Resolve and hash every backup input before stopping Handy or touching the
    # installation. Schema 1 manifests did not store hashes, so their immutable
    # backup files become the expected source of truth.
    $runtimeHashes = @{}
    foreach ($entry in $manifest.files) {
        if (-not [bool]$entry.existed) { continue }
        $backup = Join-Path $backupRoot ([string]$entry.backup_path)
        if (-not (Test-Path -LiteralPath $backup -PathType Leaf)) {
            throw "Rollback runtime backup is missing: $($entry.relative_path)"
        }
        $recorded = Get-OptionalProperty -Object $entry -Name 'original_sha256'
        $backupHash = Get-Sha256 $backup
        if ($recorded -and $backupHash -ne [string]$recorded) {
            throw "Rollback runtime backup hash is invalid: $($entry.relative_path)"
        }
        $runtimeHashes[[string]$entry.relative_path] = $backupHash
    }

    $resourceHashes = $null
    if ([bool]$manifest.resources_existed) {
        $resourceBackup = Join-Path $backupRoot 'resources'
        if (-not (Test-Path -LiteralPath $resourceBackup -PathType Container)) {
            throw 'Rollback resources backup is missing.'
        }
        $recorded = Get-OptionalProperty -Object $manifest -Name 'original_resource_hashes'
        if ($recorded) {
            Assert-TreeHashes -Root $resourceBackup -Expected $recorded -Label 'Rollback resources backup'
        }
        $resourceHashes = Get-TreeHashes $resourceBackup
    }

    $modelHash = $null
    if ([bool]$manifest.model_existed) {
        $modelBackup = Join-Path $backupRoot 'model' $ModelFileName
        if (-not (Test-Path -LiteralPath $modelBackup -PathType Leaf)) {
            throw 'Rollback model backup is missing.'
        }
        $recorded = Get-OptionalProperty -Object $manifest -Name 'original_model_sha256'
        $modelHash = Get-Sha256 $modelBackup
        if ($recorded -and $modelHash -ne [string]$recorded) {
            throw 'Rollback model backup hash is invalid.'
        }
    }

    $settingsBackup = Join-Path $backupRoot 'settings_store.json'
    if (-not (Test-Path -LiteralPath $settingsBackup -PathType Leaf)) {
        throw 'Rollback settings backup is missing.'
    }
    $recordedSettings = Get-OptionalProperty -Object $manifest -Name 'settings_sha256'
    $settingsHash = Get-Sha256 $settingsBackup
    if ($recordedSettings -and $settingsHash -ne [string]$recordedSettings) {
        throw 'Rollback settings backup hash is invalid.'
    }

    [pscustomobject]@{
        ManifestPath = $resolvedManifest
        BackupRoot = $backupRoot
        Manifest = $manifest
        TargetRoot = $targetRoot
        RuntimeHashes = $runtimeHashes
        ResourceHashes = $resourceHashes
        ModelHash = $modelHash
        SettingsHash = $settingsHash
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
    $plan = Get-RollbackPlan -ManifestPath $ManifestPath
    $backupRoot = $plan.BackupRoot
    $manifest = $plan.Manifest
    $targetRoot = $plan.TargetRoot
    Stop-HandyAt (Join-Path $targetRoot 'handy.exe')

    foreach ($entry in $manifest.files) {
        $target = Join-Path $targetRoot ([string]$entry.relative_path)
        Assert-UnderRoot -Path $target -Root $targetRoot
        if ([bool]$entry.existed) {
            Copy-Item -LiteralPath (Join-Path $backupRoot ([string]$entry.backup_path)) -Destination $target -Force
            if ((Get-Sha256 $target) -ne [string]$plan.RuntimeHashes[[string]$entry.relative_path]) {
                throw "Rollback runtime verification failed: $($entry.relative_path)"
            }
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
        Assert-TreeHashes -Root $resources -Expected $plan.ResourceHashes -Label 'Rollback resources'
    }

    $modelTarget = Join-Path $targetRoot ([string]$manifest.model_relative_path)
    Assert-UnderRoot -Path $modelTarget -Root $targetRoot
    if ([bool]$manifest.model_existed) {
        Copy-Item -LiteralPath (Join-Path $backupRoot 'model' $ModelFileName) -Destination $modelTarget -Force
        if ((Get-Sha256 $modelTarget) -ne [string]$plan.ModelHash) {
            throw 'Rollback model verification failed.'
        }
    } elseif (Test-Path -LiteralPath $modelTarget) {
        Remove-Item -LiteralPath $modelTarget -Force
    }

    Copy-Item -LiteralPath (Join-Path $backupRoot 'settings_store.json') -Destination (Join-Path $targetRoot 'Data\settings_store.json') -Force
    if ((Get-Sha256 (Join-Path $targetRoot 'Data\settings_store.json')) -ne [string]$plan.SettingsHash) {
        throw 'Rollback settings verification failed.'
    }
    Start-Process -FilePath (Join-Path $targetRoot 'handy.exe')
    Write-Output "Rollback complete: $backupRoot"
}

if ($PSCmdlet.ParameterSetName -eq 'Rollback') {
    Restore-Install -ManifestPath $RollbackFrom
    exit 0
}

if ($PSCmdlet.ParameterSetName -eq 'ValidateRollback') {
    $plan = Get-RollbackPlan -ManifestPath $ValidateRollbackFrom
    Write-Output "Rollback backup is complete and verifiable: $($plan.BackupRoot)"
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

Stop-HandyAt $installedExe
if (Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath -eq $installedExe }) {
    throw 'Installed Handy process did not stop; refusing to create a racing backup.'
}

$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$backupRoot = Join-Path (Split-Path -Parent $InstallRoot) "Handy-backups\progressive-$stamp"
try {
    New-Item -ItemType Directory -Path $backupRoot -Force | Out-Null
    Copy-Item -LiteralPath $settingsPath -Destination (Join-Path $backupRoot 'settings_store.json')

    $fileManifest = @()
    foreach ($name in $RuntimeFiles) {
        $source = Join-Path $CandidateRoot $name
        if (-not (Test-Path -LiteralPath $source)) { throw "Candidate runtime file is missing: $source" }
        $target = Join-Path $InstallRoot $name
        Assert-UnderRoot -Path $target -Root $InstallRoot
        $existed = Test-Path -LiteralPath $target
        $backupPath = "runtime\$name"
        $originalHash = if ($existed) { Get-Sha256 $target } else { $null }
        if ($existed) {
            New-Item -ItemType Directory -Path (Join-Path $backupRoot 'runtime') -Force | Out-Null
            Copy-Item -LiteralPath $target -Destination (Join-Path $backupRoot $backupPath)
        }
        $fileManifest += [ordered]@{
            relative_path = $name
            existed = $existed
            backup_path = $backupPath
            original_sha256 = $originalHash
            candidate_sha256 = Get-Sha256 $source
        }
    }

    $resourcesTarget = Join-Path $InstallRoot 'resources'
    $resourcesExisted = Test-Path -LiteralPath $resourcesTarget
    $originalResourceHashes = if ($resourcesExisted) { Get-TreeHashes $resourcesTarget } else { [ordered]@{} }
    if ($resourcesExisted) {
        Copy-Item -LiteralPath $resourcesTarget -Destination (Join-Path $backupRoot 'resources') -Recurse
    }
    $modelExisted = Test-Path -LiteralPath $installedModel
    $originalModelHash = if ($modelExisted) { Get-Sha256 $installedModel } else { $null }
    if ($modelExisted) {
        New-Item -ItemType Directory -Path (Join-Path $backupRoot 'model') -Force | Out-Null
        Copy-Item -LiteralPath $installedModel -Destination (Join-Path $backupRoot 'model' $ModelFileName)
    }

    $manifest = [ordered]@{
        schema_version = 2
        created_at = (Get-Date).ToString('o')
        install_root = $InstallRoot
        installed_exe_sha256 = Get-Sha256 $installedExe
        candidate_exe_sha256 = Get-Sha256 $candidateExe
        settings_sha256 = Get-Sha256 $settingsPath
        files = $fileManifest
        candidate_resource_hashes = Get-TreeHashes $candidateResources
        original_resource_hashes = $originalResourceHashes
        resources_existed = $resourcesExisted
        model_relative_path = "Data\models\$ModelFileName"
        model_existed = $modelExisted
        original_model_sha256 = $originalModelHash
    }
    $manifestPath = Join-Path $backupRoot 'manifest.json'
    $manifest | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $manifestPath -Encoding UTF8

    foreach ($entry in $fileManifest) {
        Copy-Item -LiteralPath (Join-Path $CandidateRoot $entry.relative_path) -Destination (Join-Path $InstallRoot $entry.relative_path) -Force
        if ((Get-Sha256 (Join-Path $InstallRoot $entry.relative_path)) -ne $entry.candidate_sha256) {
            throw "Installed runtime verification failed: $($entry.relative_path)"
        }
    }
    if (Test-Path -LiteralPath $resourcesTarget) {
        Assert-UnderRoot -Path $resourcesTarget -Root $InstallRoot
        Remove-Item -LiteralPath $resourcesTarget -Recurse -Force
    }
    Copy-Item -LiteralPath $candidateResources -Destination $resourcesTarget -Recurse
    Assert-TreeHashes -Root $resourcesTarget -Expected $manifest.candidate_resource_hashes -Label 'Installed resources'
    Copy-Item -LiteralPath $candidateModel -Destination $installedModel -Force

    $settingsDocument = Get-Content -Raw -LiteralPath $settingsPath | ConvertFrom-Json
    $settings = if ($settingsDocument.settings) { $settingsDocument.settings } else { $settingsDocument }
    Set-JsonProperty -Object $settings -Name 'selected_model' -Value $ModelId
    Set-JsonProperty -Object $settings -Name 'progressive_output_mode' -Value 'direct_prompt'
    Set-JsonProperty -Object $settings -Name 'onboarding_completed' -Value $true
    $settingsJson = $settingsDocument | ConvertTo-Json -Depth 100
    $utf8WithoutBom = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText($settingsPath, $settingsJson, $utf8WithoutBom)

    if ((Get-Sha256 $installedExe) -ne $ExpectedExeSha256.ToUpperInvariant()) { throw 'Installed executable hash verification failed.' }
    if ((Get-Sha256 $installedModel) -ne $ModelSha256) { throw 'Installed model hash verification failed.' }

    Start-Process -FilePath $installedExe
    Start-Sleep -Seconds 8
    if (-not (Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath -eq $installedExe })) {
        throw 'Installed Handy did not remain running.'
    }
} catch {
    $failure = $_
    $manifestPath = Join-Path $backupRoot 'manifest.json'
    if (Test-Path -LiteralPath $manifestPath) {
        Restore-Install -ManifestPath $manifestPath
    } elseif (Test-Path -LiteralPath $installedExe) {
        Start-Process -FilePath $installedExe
    }
    throw "Installation failed and rollback was attempted: $failure"
}

Write-Output "Installed and running. Backup: $backupRoot"
Write-Output "Rollback: $PSCommandPath -RollbackFrom '$(Join-Path $backupRoot 'manifest.json')'"
