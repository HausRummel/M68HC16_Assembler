<#
.SYNOPSIS
    Assembles a body of HC16 source through the MASM golden oracle and parses the
    listing into structured (Abs, Loc, Obj, Bytes, Source) rows — the authoritative
    encoding for each line. Use this to build the ISA encoding table and the Rust
    encoder's golden fixtures.

.DESCRIPTION
    Wraps Invoke-MasmOracle.ps1. The MASM listing uses fixed columns:
        Abs.   Loc    Obj. code   Source line
        cols:  0-3    7-12   14-22       26+
    'Loc' is the 6-hex location counter; 'Obj. code' holds up to two hex words
    ("XXXX XXXX"); longer encodings continue on following lines (merged here).

.EXAMPLE
    .\Get-MasmEncoding.ps1 -Body "        ldaa #`$12","        ldab `$40" |
        Format-Table Loc, Bytes, Source
#>
[CmdletBinding()]
param(
    # Body source lines (without org/end — those are added unless -Raw).
    [Parameter(Mandatory = $true)]
    [string[]]$Body,

    [string]$Org = '$2000',
    [switch]$Raw,                # supply a complete program; don't add org/end
    [string]$SaveListingTo,      # optional path to copy the raw .LST
    [string]$SaveAsmTo           # optional path to copy the generated IN.ASM
)

$ErrorActionPreference = 'Stop'
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$oracle = Join-Path $here 'Invoke-MasmOracle.ps1'

if ($Raw) { $src = $Body }
else { $src = @("        org $Org") + $Body + @("        end") }

$out = Join-Path ([System.IO.Path]::GetTempPath()) ("masm_enc_" + [System.IO.Path]::GetRandomFileName().Replace('.',''))
$r = & $oracle -Source $src -OutDir $out -TimeoutSec 60

$lstPath = Join-Path $out 'OUT.LST'
if (-not (Test-Path $lstPath)) { throw "No listing produced. Build likely failed; check $out\OUT.ERR" }
if ($SaveListingTo) { Copy-Item $lstPath $SaveListingTo -Force }
if ($SaveAsmTo)     { Copy-Item (Join-Path $out 'IN.ASM') $SaveAsmTo -Force }

$lines = Get-Content $lstPath
$rows = New-Object System.Collections.Generic.List[object]
$prev = $null

foreach ($ln in $lines) {
    # The code listing ends at "N lines assembled"; everything after (Symbol
    # Table, Cross Reference Table) is not object code — stop here.
    if ($ln -match 'lines assembled') { break }
    # Skip page headers, separators, blanks.
    if ($ln -match 'Motorola Macro Assembler' -or $ln -match '^\s*$') { continue }
    if ($ln -match '^Abs\.|^----') { continue }
    if ($ln.Length -lt 7) { continue }

    $absStr = $ln.Substring(0, [Math]::Min(4,$ln.Length)).Trim()
    if ($absStr -notmatch '^\d+$') {
        # Continuation line of object code (no line number): append bytes to prev row.
        if ($prev -and $ln.Length -ge 14) {
            $cont = $ln.Substring(14, [Math]::Min(9, $ln.Length-14)).Trim()
            if ($cont -match '^[0-9A-Fa-f ]+$' -and $cont.Trim() -ne '') {
                $prev.Bytes += ($cont -replace '\s','')
            }
        }
        continue
    }

    $loc = if ($ln.Length -ge 13) { $ln.Substring(7,6).Trim() } else { '' }
    $obj = if ($ln.Length -ge 14) { $ln.Substring(14,[Math]::Min(9,$ln.Length-14)).Trim() } else { '' }
    $srcTxt = if ($ln.Length -ge 26) { $ln.Substring(26).TrimEnd() } else { '' }

    $row = [pscustomobject]@{
        Abs    = [int]$absStr
        Loc    = $loc
        Bytes  = ($obj -replace '\s','')
        Source = $srcTxt
    }
    $rows.Add($row)
    $prev = $row
}

# Normalize Bytes to spaced hex pairs for readability
foreach ($row in $rows) {
    $b = $row.Bytes
    if ($b -match '^[0-9A-Fa-f]+$' -and $b.Length % 2 -eq 0 -and $b.Length -gt 0) {
        $pairs = for ($i=0; $i -lt $b.Length; $i+=2) { $b.Substring($i,2).ToUpper() }
        $row.Bytes = ($pairs -join ' ')
    }
}

Remove-Item $out -Recurse -Force -ErrorAction SilentlyContinue
$rows
