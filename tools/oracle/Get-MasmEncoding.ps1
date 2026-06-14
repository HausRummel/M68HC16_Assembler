<#
.SYNOPSIS
    Assembles a body of HC16 source through the MASM golden oracle and parses the
    listing into structured (Abs, Loc, Bytes, Source) rows — the authoritative
    encoding for each line. Use this to build the ISA encoding table and the Rust
    encoder's golden fixtures.

.DESCRIPTION
    Wraps Invoke-MasmOracle.ps1. The MASM listing column layout VARIES: with no
    errors it is "Abs. | Loc | Obj. code | Source"; once any line errors, MASM
    inserts a "Rel." column, shifting everything. So columns are detected
    dynamically from the dashed separator line rather than hardcoded.

    Handling:
      * Error lines ("Error IN.ASM N: - msg") -> skipped; the offending source
        line gets no bytes (this is how an illegal mode is detected).
      * Continuation lines (6-byte instructions wrap to a second listing line with
        the same Abs, a new Loc, more bytes, and blank source) -> bytes appended
        to the in-progress row.
      * A single error makes MASM abort the OBJ, but the LST still lists bytes for
        every line that assembled — so probe batches with illegal combos are fine.

.EXAMPLE
    .\Get-MasmEncoding.ps1 -Body '        ldaa #$12','        addd #$1234' |
        Format-Table Loc, Bytes, Source
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string[]]$Body,

    [string]$Org = '$2000',
    [switch]$Raw,
    [string]$SaveListingTo,
    [string]$SaveAsmTo
)

$ErrorActionPreference = 'Stop'
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$oracle = Join-Path $here 'Invoke-MasmOracle.ps1'

if ($Raw) { $src = $Body }
else { $src = @("        org $Org") + $Body + @("        end") }

$out = Join-Path ([System.IO.Path]::GetTempPath()) ("masm_enc_" + [System.IO.Path]::GetRandomFileName().Replace('.',''))
& $oracle -Source $src -OutDir $out -TimeoutSec 90 | Out-Null

$lstPath = Join-Path $out 'OUT.LST'
if (-not (Test-Path $lstPath)) { throw "No listing produced. See $out\OUT.ERR" }
if ($SaveListingTo) { Copy-Item $lstPath $SaveListingTo -Force }
if ($SaveAsmTo)     { Copy-Item (Join-Path $out 'IN.ASM') $SaveAsmTo -Force }

$lines = Get-Content $lstPath

# --- Locate the column layout from the dashed separator line -----------------
# Find runs of '-' and treat each as a column. Layout is either
#   [Abs, Loc, Obj, Source]  or  [Abs, Rel, Loc, Obj, Source].
$cols = $null
foreach ($ln in $lines) {
    if ($ln -match '^-{2,}(\s+-{2,})+\s*$') {
        $groups = [regex]::Matches($ln, '-{2,}')
        $spans = @()
        foreach ($g in $groups) { $spans += [pscustomobject]@{ Start = $g.Index; Len = $g.Length } }
        $n = $spans.Count
        if ($n -ge 4) {
            $cols = [ordered]@{
                Loc    = $spans[$n-3]
                Obj    = $spans[$n-2]
                SrcAt  = $spans[$n-1].Start
                AbsAt  = $spans[0].Start
                AbsLen = $spans[0].Len
            }
            break
        }
    }
}
if (-not $cols) { throw "Could not locate listing column separator." }

function Get-Field([string]$line, [int]$start, [int]$len) {
    if ($line.Length -le $start) { return '' }
    $l = [Math]::Min($len, $line.Length - $start)
    return $line.Substring($start, $l).Trim()
}

$rows = New-Object System.Collections.Generic.List[object]
$prev = $null

foreach ($ln in $lines) {
    # End of the code listing: success prints "N lines assembled"; an aborted
    # build prints "Aborted assembly"; both precede the "Symbol Table:" section.
    if ($ln -match 'lines assembled' -or $ln -match 'Aborted assembly' -or $ln -match 'Symbol Table:') { break }
    if ($ln -match '^(Error|Warning|Copyright|68HC16)') { continue }   # banners / sub-header / diagnostics
    if ($ln -match 'Macro Assembler' -or $ln -match '^\s*$') { continue }
    if ($ln -match '^Abs\.|^-{2,}\s') { continue }

    $abs = Get-Field $ln $cols.AbsAt $cols.AbsLen
    $loc = Get-Field $ln $cols.Loc.Start $cols.Loc.Len
    $obj = (Get-Field $ln $cols.Obj.Start $cols.Obj.Len) -replace '\s',''
    $srcTxt = if ($ln.Length -gt $cols.SrcAt) { $ln.Substring($cols.SrcAt).TrimEnd() } else { '' }

    $hasLoc = $loc -match '^[0-9A-Fa-f]{4,6}$'
    $hasObj = $obj -match '^[0-9A-Fa-f]+$' -and $obj -ne ''

    if ($hasLoc -and $hasObj -and $srcTxt -eq '' -and $prev) {
        # Continuation line: append bytes to the in-progress instruction.
        $prev.Raw += $obj
        continue
    }

    # A normal source line (may or may not carry object code).
    $row = [pscustomobject]@{
        Abs    = if ($abs -match '^\d+$') { [int]$abs } else { $null }
        Loc    = if ($hasLoc) { $loc.ToUpper() } else { '' }
        Raw    = if ($hasObj) { $obj } else { '' }
        Bytes  = ''
        Source = $srcTxt
    }
    $rows.Add($row)
    $prev = $row
}

# Normalize Raw hex -> spaced byte pairs.
foreach ($row in $rows) {
    $b = $row.Raw
    if ($b -and $b.Length % 2 -eq 0) {
        $pairs = for ($i=0; $i -lt $b.Length; $i+=2) { $b.Substring($i,2).ToUpper() }
        $row.Bytes = ($pairs -join ' ')
    } else {
        $row.Bytes = $b.ToUpper()
    }
}

Remove-Item $out -Recurse -Force -ErrorAction SilentlyContinue
$rows | Select-Object Abs, Loc, Bytes, Source
