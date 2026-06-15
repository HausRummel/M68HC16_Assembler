        org $2000
        pshm d,e
        pulm x,y
        pshm d,e,x,y,z,k,ccr
        movb $1000,$2000
        movb $08,x,$2000
        movb $2000,$08,x
        movw $1000,$2000
        rmac $04,$06
        ldaa #'Q'
        fcb 'A','B',$0d
        end

