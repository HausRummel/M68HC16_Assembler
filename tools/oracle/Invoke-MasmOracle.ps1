<#
.SYNOPSIS
    Golden-oracle runner: assembles an .asm file with the ORIGINAL Motorola
    MASM 4.6 toolchain under DOSBox and collects the OBJ / LST / S19 / ERR
    outputs, so the new Rust assembler can be validated byte-for-byte.

.DESCRIPTION
    Builds a hermetic work directory containing a copy of the Motorola toolchain
    (Masm.exe, Dos4gw.exe, Hex.exe, Ld.exe) plus the input source, generates a
    throwaway DOSBox config whose [autoexec] runs the exact build the original
    project used (Asmb.bat: `MASM -a -x -t -o OUT.OBJ -l OUT.LST IN.ASM` then
    `HEX OUT.OBJ > OUT.S19`), runs DOSBox headless-ish, and copies the artifacts
    back out.

    DOSBox 0.74 is 8.3-filenames only, so inside the box the input is always
    IN.ASM and outputs are OUT.*. Original long names are irrelevant to the bytes.

.EXAMPLE
    # Validate environment only
    .\Invoke-MasmOracle.ps1 -CheckEnv

.EXAMPLE
    # Assemble inline source, keep outputs in .\out
    .\Invoke-MasmOracle.ps1 -Source "        org `$2000","        ldaa #`$12","        end" -OutDir .\out

.EXAMPLE
    # Assemble a standalone file
    .\Invoke-MasmOracle.ps1 -InputAsm .\snippet.asm -OutDir .\out
#>
[CmdletBinding(DefaultParameterSetName = 'File')]
param(
    [Parameter(ParameterSetName = 'File', Mandatory = $true)]
    [string]$InputAsm,

    [Parameter(ParameterSetName = 'Inline', Mandatory = $true)]
    [string[]]$Source,

    [Parameter(ParameterSetName = 'CheckEnv', Mandatory = $true)]
    [switch]$CheckEnv,

    [string]$OutDir,

    # Extra directory whose files (e.g. includes) are copied into the work dir.
    [string]$IncludeDir,

    # Exact MASM flag string (mirrors Asmb.bat). Override only if you know why.
    [string]$MasmArgs = '-a -x -t -o OUT.OBJ -l OUT.LST IN.ASM',

    # Paths are resolved from (in order): these params, env vars
    # HC16_DOSBOX / HC16_MASM_TOOLCHAIN, then a local gitignored config
    # (oracle.private.psd1). The toolchain path is kept out of git on purpose
    # (it lives in an isolated location); see oracle.config.example.psd1.
    [string]$DosBox,
    [string]$Toolchain,
    [string]$ConfigFile,

    [int]$TimeoutSec = 60,
    [switch]$KeepWork,
    [switch]$DryRun
)

$ErrorActionPreference = 'Stop'

# ---- Resolve tool paths (param > env > local config) ------------------------
function Coalesce { foreach ($v in $args) { if ($v -and "$v".Trim() -ne '') { return $v } } return $null }

if (-not $ConfigFile) { $ConfigFile = Join-Path $PSScriptRoot 'oracle.private.psd1' }
$cfg = @{}
if (Test-Path $ConfigFile) {
    try { $cfg = Import-PowerShellDataFile $ConfigFile } catch { throw "Failed to read config '$ConfigFile': $_" }
}

$DosBox    = Coalesce $DosBox    $env:HC16_DOSBOX         $cfg.DosBox    'C:\Program Files (x86)\DOSBox-0.74-3\DOSBox.exe'
$Toolchain = Coalesce $Toolchain $env:HC16_MASM_TOOLCHAIN $cfg.Toolchain
if (-not $Toolchain) {
    throw "Toolchain path not set. Provide -Toolchain, set `$env:HC16_MASM_TOOLCHAIN, or create $ConfigFile (copy oracle.config.example.psd1)."
}

function Resolve-ToolFile([string]$dir, [string]$name) {
    $p = Join-Path $dir $name
    if (-not (Test-Path $p)) { throw "Required tool '$name' not found in '$dir'." }
    return (Get-Item $p).FullName
}

# ---- Environment validation -------------------------------------------------
$env_ok = $true
$report = [ordered]@{}

$report['DOSBox']    = if (Test-Path $DosBox)    { $DosBox }    else { $env_ok = $false; "MISSING: $DosBox" }
$report['Toolchain'] = if (Test-Path $Toolchain) { $Toolchain } else { $env_ok = $false; "MISSING: $Toolchain" }

foreach ($t in 'Masm.exe','Dos4gw.exe','Hex.exe','Ld.exe') {
    $tp = Join-Path $Toolchain $t
    $report["tool:$t"] = if (Test-Path $tp) { 'ok' } else { $env_ok = $false; 'MISSING' }
}

if ($CheckEnv) {
    $report.GetEnumerator() | ForEach-Object { '{0,-14}: {1}' -f $_.Key, $_.Value }
    Write-Output ("ENV: " + ($(if ($env_ok) { 'OK' } else { 'NOT READY' })))
    if (-not $env_ok) {
        Write-Output "Hint: install DOSBox-X via 'winget install dosbox-x' or fix the -Toolchain path."
    }
    return
}

if (-not $env_ok) {
    $report.GetEnumerator() | ForEach-Object { '{0,-14}: {1}' -f $_.Key, $_.Value }
    throw "Environment not ready. Run with -CheckEnv for details."
}

# ---- Build hermetic work directory -----------------------------------------
$work = Join-Path ([System.IO.Path]::GetTempPath()) ("masm_oracle_" + [System.IO.Path]::GetRandomFileName().Replace('.',''))
New-Item -ItemType Directory -Path $work -Force | Out-Null

try {
    foreach ($t in 'Masm.exe','Dos4gw.exe','Hex.exe','Ld.exe') {
        Copy-Item (Resolve-ToolFile $Toolchain $t) -Destination $work
    }

    # Place input as IN.ASM
    if ($PSCmdlet.ParameterSetName -eq 'Inline') {
        # Write CRLF, ASCII — matches original DOS sources
        Set-Content -Path (Join-Path $work 'IN.ASM') -Value $Source -Encoding ascii
    } else {
        if (-not (Test-Path $InputAsm)) { throw "InputAsm not found: $InputAsm" }
        Copy-Item (Get-Item $InputAsm).FullName -Destination (Join-Path $work 'IN.ASM')
    }

    if ($IncludeDir) {
        if (-not (Test-Path $IncludeDir)) { throw "IncludeDir not found: $IncludeDir" }
        Copy-Item (Join-Path $IncludeDir '*') -Destination $work -Recurse -Force
    }

    # ---- Generate DOSBox config --------------------------------------------
    $conf = Join-Path $work 'oracle.conf'
    $confText = @"
[sdl]
windowresolution=640x480
output=surface
[dosbox]
machine=svga_s3
[cpu]
core=auto
cputype=auto
cycles=max
[autoexec]
mount c "$work"
c:
Masm $MasmArgs
Hex OUT.OBJ > OUT.S19
exit
"@
    Set-Content -Path $conf -Value $confText -Encoding ascii

    if ($DryRun) {
        Write-Output "=== work dir: $work ==="
        Write-Output "=== oracle.conf ==="
        Get-Content $conf
        Write-Output "=== IN.ASM ==="
        Get-Content (Join-Path $work 'IN.ASM')
        Write-Output "(DryRun: DOSBox not launched; work dir kept)"
        return
    }

    # ---- Run DOSBox with timeout -------------------------------------------
    $proc = Start-Process -FilePath $DosBox -ArgumentList @('-conf', "`"$conf`"") -PassThru
    if (-not $proc.WaitForExit($TimeoutSec * 1000)) {
        try { $proc.Kill() } catch {}
        throw "DOSBox timed out after ${TimeoutSec}s (killed). Check for an interactive prompt in the build."
    }

    # ---- Collect outputs ----------------------------------------------------
    if (-not $OutDir) { $OutDir = Join-Path (Get-Location) 'oracle-out' }
    New-Item -ItemType Directory -Path $OutDir -Force | Out-Null

    $artifacts = [ordered]@{}
    foreach ($a in 'IN.ASM','OUT.OBJ','OUT.LST','OUT.S19','OUT.ERR') {
        $src = Join-Path $work $a
        if (Test-Path $src) {
            $dst = Join-Path $OutDir $a
            Copy-Item $src $dst -Force
            $artifacts[$a] = (Get-Item $dst).FullName
        }
    }

    $result = [pscustomobject]@{
        WorkDir   = $work
        OutDir    = (Get-Item $OutDir).FullName
        Artifacts = $artifacts
        ObjBytes  = if ($artifacts['OUT.OBJ']) { (Get-Item $artifacts['OUT.OBJ']).Length } else { $null }
        S19Bytes  = if ($artifacts['OUT.S19']) { (Get-Item $artifacts['OUT.S19']).Length } else { $null }
        Assembled = [bool]$artifacts['OUT.OBJ']
    }

    Write-Output $result
}
finally {
    if (-not $KeepWork -and -not $DryRun) {
        Remove-Item $work -Recurse -Force -ErrorAction SilentlyContinue
    } elseif ($KeepWork) {
        Write-Output "Work dir kept: $work"
    }
}
