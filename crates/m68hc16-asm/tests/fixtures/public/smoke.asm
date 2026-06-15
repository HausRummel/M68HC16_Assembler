        org $2000
VAL     equ $1234
start   ldaa #$12
        ldab #$34
        adda #$10
        addd #$1234
        ldd #VAL
        ldx #$abcd
        ldaa $1234
        staa $2000
        ldd $3000
        ldaa $10,x
        ldab 0,x
        ldd $20,y
        ldaa e,x
loop    bra loop
        beq start
        lbra start
        bset $40,#$01
        brclr $40,#$02,loop
        jsr start
        jmp start
        rts
        fcb $de,$ad,$5a
        fdb $beef,VAL
        end

