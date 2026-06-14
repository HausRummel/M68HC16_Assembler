@{
    # Template for the MASM golden-oracle config.
    #
    # Copy this file to `oracle.private.psd1` (which is gitignored) and fill in the
    # real paths for your machine. The original Motorola toolchain folder must
    # contain: Masm.exe, Dos4gw.exe, Hex.exe, Ld.exe.
    #
    # Alternatively, set environment variables HC16_DOSBOX and
    # HC16_MASM_TOOLCHAIN, or pass -DosBox / -Toolchain to Invoke-MasmOracle.ps1.

    DosBox    = 'C:\Program Files (x86)\DOSBox-0.74-3\DOSBox.exe'
    Toolchain = 'X:\path\to\original\masm\toolchain\folder'
}
