        org $2000
        bset $40,z,#$01
        bclr $42,z,#$02
        bset $1234,#$04
        brset $40,x,#$08,*
        brclr $44,y,#$10,*
        brset $1234,#$20,*
        bset $10,x,#$80
        end

