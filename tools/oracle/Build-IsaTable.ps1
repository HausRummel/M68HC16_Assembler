<#
.SYNOPSIS
    Classifies the raw ISA probe into a canonical encoding table by differential
    probing: each (mnemonic, mode) is assembled with TWO distinct operand values;
    constant leading bytes = opcode prefix, trailing changing bytes = operand.

.DESCRIPTION
    Robust against per-mnemonic operand widths (e.g. jmp's indexed offset is 16-bit
    even though an 8-bit value zero-extends and "looks" 8-bit). Collapse rules:
      * ext vs ext20 with equal total length -> same mode (keep the wider operand).
      * ind8 vs ind16 (per index reg) with equal total length -> same 16-bit mode
        (the 8-bit value just zero-extended); different lengths -> two real modes.
      * rel is accepted only for pure-branch mnemonics (no value-bearing mode).

    Output: docs/spec/isa-table.tsv (mnemonic, mode, prefix_hex, operand_len, total).
#>
[CmdletBinding()]
param(
    [string[]]$Mnemonics,
    [string]$OutTsv = "$PSScriptRoot\..\..\docs\spec\isa-table.tsv",
    [int]$ChunkSize = 90
)

$ErrorActionPreference = 'Stop'
$enc = Join-Path $PSScriptRoot 'Get-MasmEncoding.ps1'
$specDir = Join-Path $PSScriptRoot '..\..\docs\spec'

# Canonical mode probes. k = kind: 'single' (no operand variance), 'diff' (a&b),
# 'rel' (branch-only, classified by length). reg = index register where relevant.
$MODES = @(
    @{ n='inh';    k='single'; a='' },
    @{ n='imm8';   k='diff';   a='#$05';        b='#$3A' },
    @{ n='imm16';  k='diff';   a='#$1234';      b='#$5A6C' },
    @{ n='ext';    k='diff';   a='$1234';       b='$5A6C' },
    @{ n='ext20';  k='diff';   a='$F1234';      b='$5A6C7' },
    @{ n='ind8x';  k='diff';   a='$12,x';       b='$3A,x' },
    @{ n='ind8y';  k='diff';   a='$12,y';       b='$3A,y' },
    @{ n='ind8z';  k='diff';   a='$12,z';       b='$3A,z' },
    @{ n='ind16x'; k='diff';   a='$1234,x';     b='$5A6C,x' },
    @{ n='ind16y'; k='diff';   a='$1234,y';     b='$5A6C,y' },
    @{ n='ind16z'; k='diff';   a='$1234,z';     b='$5A6C,z' },
    @{ n='eindx';  k='single'; a='e,x' },
    @{ n='eindy';  k='single'; a='e,y' },
    @{ n='eindz';  k='single'; a='e,z' },
    @{ n='rel';    k='rel';    a='*' },
    @{ n='bit';    k='diff';   a='$1234,#$5A';     b='$6C78,#$3E' },
    @{ n='bitbr';  k='diff';   a='$1234,#$5A,*';   b='$6C78,#$3E,*' },
    @{ n='reg';    k='diff';   a='d,e';         b='x,y' }
)

# NOTE: passing -Mnemonics explicitly probes just that set (write to a scratch
# -OutTsv and MERGE into docs/spec/isa-table.tsv — that canonical table is
# hand-maintained and carries modes the differential probe cannot reach:
# BitIndW (bsetw/bclrw), Mac (mac/rmac), mov_*, ind20, and the bit-indexed forms.
# The no-arg default below seeds only mnemonics that appear in OUR corpus; that
# corpus-subset scoping is exactly what left TSX/PSHX/NEG (and ~60 other valid
# HC16 ops) out until a different source release exercised them.
if (-not $Mnemonics) {
    # 'bsz' (block-storage-zeros) is a directive, not an opcode: probing it emits a
    # bogus all-zero "encoding" while the location counter jumps by the operand.
    $DIRECTIVES = @('org','equ','set','fcb','fdb','fcc','rmb','dc','ds','dcb','bsz','end','page','plen','ttl','title','spc','tabs','llen','nol','nolist','list','nopage','newpage','opt','include','incbin','asct','bsct','psct','dsct','csct','idsct','ipsct','section','base','align','even','longeven','mlist','alist','clist','file','fail','macro','endm','mexit','if','ifc','ifnc','ifdef','ifndef','ifeq','ifne','ifgt','iflt','ifge','ifle','else','elsec','endc','endi','endf','while','repeat','endw','endr','exitm','regdef','lreg','xdef','xref','xrefb','global','extern','public','common','comment','offset','struct','ends','reg','sttl')
    $table=@{}; Get-Content (Join-Path $specDir 'masm-mnemonic-table.tsv') | Select-Object -Skip 1 | ForEach-Object { $c=$_ -split "`t"; if($c.Count -ge 5){$table[$c[4]]=$true} }
    $corpus=@(); Get-Content (Join-Path $specDir 'corpus-op-frequency.tsv') | Select-Object -Skip 1 | ForEach-Object { $c=$_ -split "`t"; if($c.Count -ge 1){$corpus+=$c[0]} }
    $Mnemonics = $corpus | Where-Object { $table.ContainsKey($_) -and ($DIRECTIVES -notcontains $_) } | Sort-Object -Unique
}

function ToBytes([string]$hexSpaced) { if (-not $hexSpaced) { return @() } return @($hexSpaced -split ' ' | Where-Object {$_ -ne ''}) }
function FirstDiff($a, $b) { $n=[Math]::Min($a.Count,$b.Count); for($i=0;$i -lt $n;$i++){ if($a[$i] -ne $b[$i]){return $i} }; return $n }

Write-Host ("Differential-probing {0} mnemonics x {1} modes, chunks of {2}." -f $Mnemonics.Count, $MODES.Count, $ChunkSize)

# results[mnemonic][mode] = @{ a=bytes[]; b=bytes[] }
$results = @{}
$chunks = [Math]::Ceiling($Mnemonics.Count / $ChunkSize)
for ($ci=0; $ci -lt $chunks; $ci++) {
    $slice = $Mnemonics[($ci*$ChunkSize)..([Math]::Min(($ci+1)*$ChunkSize-1, $Mnemonics.Count-1))]
    $body = New-Object System.Collections.Generic.List[string]
    $meta = @{}
    $body.Add('        org $2000') | Out-Null
    foreach ($mn in $slice) {
        if (-not $results.ContainsKey($mn)) { $results[$mn] = @{} }
        foreach ($M in $MODES) {
            foreach ($which in @('a','b')) {
                if ($which -eq 'b' -and $M.k -ne 'diff') { continue }
                $op = if ($which -eq 'a') { $M.a } else { $M.b }
                $line = if ($op -eq '') { "        $mn" } else { "        $mn $op" }
                $body.Add($line) | Out-Null
                $meta[$body.Count] = @{ mn=$mn; mode=$M.n; which=$which }
            }
        }
    }
    $body.Add('        end') | Out-Null
    Write-Host ("  chunk {0}/{1}: {2} mnemonics, {3} lines..." -f ($ci+1),$chunks,$slice.Count,$body.Count)
    $rows = & $enc -Body $body -Raw
    foreach ($r in $rows) {
        if (-not $r.Bytes) { continue }
        if (-not $meta.ContainsKey($r.Abs)) { continue }
        $m = $meta[$r.Abs]
        if (-not $results[$m.mn].ContainsKey($m.mode)) { $results[$m.mn][$m.mode] = @{} }
        $results[$m.mn][$m.mode][$m.which] = ToBytes $r.Bytes
    }
}

# ---- Classify each mnemonic's modes -----------------------------------------
$EXT_FED   = @('12','34')        # bytes of the ext probe value   ($1234)
$EXT20_FED = @('0F','12','34')   # bytes of the ext20 probe value ($F1234)
$out = New-Object System.Collections.Generic.List[object]
foreach ($mn in @($results.Keys | Sort-Object)) {
    $mModes = $results[$mn]
    $entries = @{}   # modeName -> @{ prefix=bytes[]; oplen=int; total=int }

    # Does this mnemonic accept any value-bearing (non-rel) mode? -> not a pure branch.
    $valueBearing = $false

    foreach ($M in $MODES) {
        $d = $mModes[$M.n]
        if (-not $d) { continue }
        switch ($M.k) {
            'single' {
                if ($d.a) {
                    $entries[$M.n] = @{ prefix=$d.a; oplen=0; total=$d.a.Count; a=$d.a }
                    if ($M.n -ne 'inh') { $valueBearing = $true }
                }
            }
            'diff' {
                if ($d.a -and $d.b -and $d.a.Count -eq $d.b.Count -and $d.a.Count -gt 0) {
                    $fd = FirstDiff $d.a $d.b
                    $prefix = if ($fd -gt 0) { $d.a[0..($fd-1)] } else { @() }
                    $oplen = $d.a.Count - $fd
                    $entries[$M.n] = @{ prefix=$prefix; oplen=$oplen; total=$d.a.Count; a=$d.a }
                    $valueBearing = $true
                }
            }
            'rel' { }  # handled after, only for pure branches
        }
    }

    # rel only if pure branch (no value-bearing mode matched).
    if (-not $valueBearing -and $mModes['rel'] -and $mModes['rel'].a) {
        $bytes = $mModes['rel'].a
        $plen = if ($bytes[0] -eq '37') { 2 } else { 1 }
        $oplen = $bytes.Count - $plen
        $name = if ($oplen -le 1) { 'rel8' } else { 'rel16' }
        $entries[$name] = @{ prefix=$bytes[0..($plen-1)]; oplen=$oplen; total=$bytes.Count; a=$bytes }
    }

    # (A) Inherent: if the bare-mnemonic form assembled, the op ignores operands;
    #     every other entry with the same bytes is a duplicate -> drop them.
    if ($entries.ContainsKey('inh')) {
        $inhPfx = ($entries['inh'].prefix -join ' ')
        foreach ($k in @($entries.Keys)) {
            if ($k -ne 'inh' -and ($entries[$k].prefix -join ' ') -eq $inhPfx) { $entries.Remove($k) }
        }
    }

    # (B) An ext/ext20 form whose emitted operand != the fed address is actually a
    #     PC-relative long branch (MASM emits an offset) -> relabel rel16.
    foreach ($exm in @('ext','ext20')) {
        if (-not $entries.ContainsKey($exm)) { continue }
        $e = $entries[$exm]
        $fed = if ($exm -eq 'ext') { $EXT_FED } else { $EXT20_FED }
        if ($e.oplen -eq $fed.Count -and $e.a.Count -ge $e.oplen) {
            $operand = $e.a[($e.a.Count - $e.oplen)..($e.a.Count - 1)]
            if (($operand -join '') -ne ($fed -join '')) {
                $entries.Remove($exm)
                $entries['rel16'] = @{ prefix=$e.prefix; oplen=$e.oplen; total=$e.total; a=$e.a }
            }
        }
    }

    # (C) ext vs ext20 (equal total => same mode; keep the wider operand).
    if ($entries.ContainsKey('ext') -and $entries.ContainsKey('ext20') -and
        $entries['ext'].total -eq $entries['ext20'].total) {
        # Wider operand => genuine 20-bit; a tie means the 20-bit probe truncated to
        # 16-bit, so keep the 'ext' (16-bit) label.
        if ($entries['ext20'].oplen -gt $entries['ext'].oplen) { $entries.Remove('ext') } else { $entries.Remove('ext20') }
    }
    # (D) imm8 vs imm16 (equal total => 8-bit value zero-extended; keep imm16).
    if ($entries.ContainsKey('imm8') -and $entries.ContainsKey('imm16') -and
        $entries['imm8'].total -eq $entries['imm16'].total) { $entries.Remove('imm8') }
    # (E) ind8r vs ind16r per register (equal total => keep the 16-bit one).
    foreach ($r in 'x','y','z') {
        $i8="ind8$r"; $i16="ind16$r"
        if ($entries.ContainsKey($i8) -and $entries.ContainsKey($i16) -and
            $entries[$i8].total -eq $entries[$i16].total) { $entries.Remove($i8) }
    }
    # (F) Register-list ops (pshm/pulm): "e,x" parses as a register list, not E-indexed.
    if ($entries.ContainsKey('reg')) { foreach ($k in @('eindx','eindy','eindz')) { [void]$entries.Remove($k) } }

    foreach ($k in ($entries.Keys | Sort-Object)) {
        $e = $entries[$k]
        $out.Add([pscustomobject]@{
            Mnemonic = $mn
            Mode     = $k
            Prefix   = ($e.prefix -join ' ')
            OpLen    = $e.oplen
            Total    = $e.total
        }) | Out-Null
    }
}

$dst = (Resolve-Path -LiteralPath (Split-Path $OutTsv)).Path + '\' + (Split-Path $OutTsv -Leaf)
"mnemonic`tmode`tprefix`toperand_len`ttotal" | Set-Content -Encoding utf8 $dst
$out | Sort-Object Mnemonic, Mode | ForEach-Object { "$($_.Mnemonic)`t$($_.Mode)`t$($_.Prefix)`t$($_.OpLen)`t$($_.Total)" } | Add-Content -Encoding utf8 $dst
$covered = ($out | Select-Object -ExpandProperty Mnemonic -Unique).Count
Write-Host ("Done. {0} mode-entries across {1} mnemonics. Saved: {2}" -f $out.Count, $covered, $dst)
$out
