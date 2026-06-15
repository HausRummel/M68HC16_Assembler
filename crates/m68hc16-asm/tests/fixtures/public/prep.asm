        org $2000
SET2:   macro
        ldaa #\1
        ldab #\2
        endm
        SET2 $11,$22
        ifgt 5-3
        ldx #$aaaa
        elsec
        ldx #$bbbb
        endc
        ifeq 1-1
        ldy #$cccc
        endc
        ifne 0
        ldx #$dddd
        endc
        abx
        aba
        end

