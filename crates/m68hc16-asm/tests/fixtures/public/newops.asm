        org $2000
* Instructions absent from our original corpus but valid HC16 (oracle-probed).
* Covers Daniel's TSX/PSHX/NEG plus the full sibling set added with them.
* --- memory read-modify-write: byte (neg) + word (negw/comw) ---
        neg  $1234
        neg  $12,x
        neg  $34,y
        neg  $56,z
        neg  $1234,x
        neg  $5678,y
        neg  $7abc,z
        negw $1234
        negw $1234,x
        comw $2244
        comw $1234,y
* --- shifts/rotates on memory: asr/lsl/rol/ror (byte + word) ---
        asr  $1234
        asr  $12,x
        asr  $1234,z
        asrw $1234
        asrw $1234,y
        lsl  $40,x
        lslw $1234,x
        rol  $1234
        rol  $20,y
        rolw $1234,z
        ror  $1234
        ror  $30,z
* --- DSP/MAC inherent + packed mac ---
        asrm
        lslm
        pshmac
        pulmac
        mac  0,0
        mac  1,2
        mac  -8,7
* --- stack-pointer / index transfers (Daniel's TSX) ---
        tsx
        tsz
        txs
        tys
        tzs
        tyz
        tzx
        tzy
* --- K-register / extension transfers ---
        tpd
        tdp
        tedm
        tmxed
        tekb
        tskb
        tykb
        tzkb
* --- push/pull X (Daniel's PSHX) ---
        pshx
        pulx
* --- SP/index adjust convenience forms ---
        ais  #$10
        ais  #$1234
        des
        dey
        ins
        iny
* --- compare SP / Z ---
        cps  #$1234
        cps  $1234
        cps  $40,x
        cpz  #$1234
        cpz  $1234,y
        cmpz $50,z
* --- store SP / Z / E:D ---
        sts  $1234
        sts  $12,x
        stz  $1234,y
        sted $2000
* --- subtract-with-carry B (imm/ext/indexed/E-indexed) ---
        sbcb #$05
        sbcb $1234
        sbcb $12,x
        sbcb $1234,y
        sbcb e,x
* --- misc inherent ---
        daa
        swi
        stop
        lpstop
        wai
        tap
        bgnd
* --- branches: rel8 (brn/bhs/blo) + rel16 long branches ---
loop    nop
        brn  loop
        bhs  loop
        blo  loop
        lbhs loop
        lblo loop
        lbrn loop
        lbvc loop
        lbvs loop
        lbev loop
        lbmv loop
        end
