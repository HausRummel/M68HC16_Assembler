<#
.SYNOPSIS
    Builds the authoritative HC16 encoding matrix by probing the real MASM with
    every (mnemonic x addressing-mode) combination and recording what assembles.

.DESCRIPTION
    For each mnemonic, emits one source line per operand template. The whole batch
    is assembled once through the golden oracle; MASM rejects illegal combinations
    (they get no bytes) and encodes legal ones. Each listing row is mapped back to
    its (mnemonic, mode) by the listing line number (Abs), which equals the 1-based
    line index in the generated source.

    The operand TEMPLATE is only a stimulus; the true addressing mode is whatever
    the resulting opcode/prebyte encodes. We record (mnemonic, template, bytes) as
    raw ground truth; mode classification from the bytes is a downstream step.

.OUTPUTS
    Writes a TSV (mnemonic, template, bytes, nbytes) and returns the rows.
#>
[CmdletBinding()]
param(
    [string[]]$Mnemonics,                  # default: instructions = (corpus ∩ table) - directives
    [string]$OutTsv = "$PSScriptRoot\..\..\docs\spec\isa-probe.tsv",
    [int]$ChunkSize = 120                  # mnemonics per DOSBox run (keeps listings sane)
)

$ErrorActionPreference = 'Stop'
$enc = Join-Path $PSScriptRoot 'Get-MasmEncoding.ps1'
$specDir = Join-Path $PSScriptRoot '..\..\docs\spec'

# Operand templates: label = probe-mode name, op = operand text ('' = inherent).
$TEMPLATES = @(
    @{ m='inh';   op='' },
    @{ m='imm8';  op='#$05' },
    @{ m='imm16'; op='#$1234' },
    @{ m='dp';    op='<$08' },
    @{ m='dir';   op='$08' },
    @{ m='ext';   op='$1234' },
    @{ m='ext20'; op='$12345' },
    @{ m='ix8';   op='$08,x' },
    @{ m='ix16';  op='$1234,x' },
    @{ m='iy8';   op='$08,y' },
    @{ m='iz8';   op='$08,z' },
    @{ m='eind';  op='e,x' },
    @{ m='rel';   op='*' },          # branch to current location: always in range
    @{ m='bit';   op='$08,#$01' },
    @{ m='bitbr'; op='$08,#$01,*' },
    @{ m='reg';   op='d,e' }
)

# Default mnemonic set: corpus-used operations that exist in the MASM table,
# minus assembler directives (kept in a stoplist) — i.e. real instructions.
if (-not $Mnemonics) {
    $DIRECTIVES = @(
        'org','equ','set','fcb','fdb','fcc','rmb','dc','ds','dcb','end','page','plen',
        'ttl','title','spc','tabs','llen','nol','nolist','list','nopage','newpage','opt',
        'include','incbin','asct','bsct','psct','dsct','csct','idsct','ipsct','section',
        'base','align','even','longeven','mlist','alist','clist','file','fail','macro',
        'endm','mexit','if','ifc','ifnc','ifdef','ifndef','ifeq','ifne','ifgt','iflt',
        'ifge','ifle','else','elsec','endc','endi','endf','while','repeat','endw','endr',
        'exitm','regdef','lreg','xdef','xref','xrefb','global','extern','public','common',
        'comment','offset','struct','ends','reg','sttl'
    )
    $table = @{}
    Get-Content (Join-Path $specDir 'masm-mnemonic-table.tsv') | Select-Object -Skip 1 | ForEach-Object {
        $c = $_ -split "`t"; if ($c.Count -ge 5) { $table[$c[4]] = $true }
    }
    $corpus = @()
    Get-Content (Join-Path $specDir 'corpus-op-frequency.tsv') | Select-Object -Skip 1 | ForEach-Object {
        $c = $_ -split "`t"; if ($c.Count -ge 1) { $corpus += $c[0] }
    }
    $Mnemonics = $corpus | Where-Object { $table.ContainsKey($_) -and ($DIRECTIVES -notcontains $_) } | Sort-Object -Unique
}

Write-Host ("Probing {0} mnemonics x {1} templates = {2} lines, in chunks of {3}." -f `
    $Mnemonics.Count, $TEMPLATES.Count, ($Mnemonics.Count * $TEMPLATES.Count), $ChunkSize)

$all = New-Object System.Collections.Generic.List[object]
$chunks = [Math]::Ceiling($Mnemonics.Count / $ChunkSize)

for ($ci = 0; $ci -lt $chunks; $ci++) {
    $slice = $Mnemonics[($ci*$ChunkSize)..([Math]::Min(($ci+1)*$ChunkSize-1, $Mnemonics.Count-1))]

    # Build body + per-line metadata keyed by 1-based source line number (= Abs).
    $body = New-Object System.Collections.Generic.List[string]
    $meta = @{}
    $body.Add('        org $2000') | Out-Null           # line 1
    $body.Add('lblF    rts')       | Out-Null           # line 2 (backward branch target)
    foreach ($mn in $slice) {
        foreach ($t in $TEMPLATES) {
            $line = if ($t.op -eq '') { "        $mn" } else { "        $mn $($t.op)" }
            $body.Add($line) | Out-Null
            $meta[$body.Count] = [pscustomobject]@{ Mnemonic = $mn; Mode = $t.m; Template = $t.op }
        }
    }
    $body.Add('        end') | Out-Null

    Write-Host ("  chunk {0}/{1}: {2} mnemonics, {3} lines..." -f ($ci+1), $chunks, $slice.Count, $body.Count)
    $rows = & $enc -Body $body -Raw

    foreach ($r in $rows) {
        if (-not $r.Bytes) { continue }
        if (-not ($meta.ContainsKey($r.Abs))) { continue }   # skip org/lblF/end
        $m = $meta[$r.Abs]
        $nbytes = ($r.Bytes -split ' ').Count
        $all.Add([pscustomobject]@{
            Mnemonic = $m.Mnemonic
            Mode     = $m.Mode
            Template = $m.Template
            Bytes    = $r.Bytes
            NBytes   = $nbytes
        }) | Out-Null
    }
}

# Dedup identical (mnemonic,bytes): collapses inherent ops (operand ignored) and
# modes that encode identically (e.g. dp vs dir both -> extended). Modes merged.
$dedup = $all | Group-Object { "$($_.Mnemonic)|$($_.Bytes)" } | ForEach-Object {
    $g = $_.Group
    [pscustomobject]@{
        Mnemonic = $g[0].Mnemonic
        Modes    = (($g | Select-Object -ExpandProperty Mode -Unique) -join ',')
        Bytes    = $g[0].Bytes
        NBytes   = [int]$g[0].NBytes
    }
} | Sort-Object Mnemonic, NBytes, Bytes

# Save TSV
$dst = (Resolve-Path -LiteralPath (Split-Path $OutTsv)).Path + '\' + (Split-Path $OutTsv -Leaf)
"mnemonic`tmodes`tbytes`tnbytes" | Set-Content -Encoding utf8 $dst
$dedup | ForEach-Object { "$($_.Mnemonic)`t$($_.Modes)`t$($_.Bytes)`t$($_.NBytes)" } | Add-Content -Encoding utf8 $dst

$covered = ($dedup | Select-Object -ExpandProperty Mnemonic -Unique).Count
Write-Host ("Done. {0} distinct encodings across {1}/{2} mnemonics. Saved: {3}" -f $dedup.Count, $covered, $Mnemonics.Count, $dst)
$dedup
