ENTRY(_start)
MEMORY {
    /* Stay inside the sectors erased by the stock J1196J updater. */
    IMAGE (rx) : ORIGIN = DEFINED(__ram_trial) ? 0x40001000 : 0,
                 LENGTH = DEFINED(__ram_trial) ? 24K : 64K
    /* Leave the bottom 0x200 and top 0x120 bytes to the bootloader. */
    LOW (rw) : ORIGIN = 0x40000200, LENGTH = 0xe00
    RAM (rw) : ORIGIN = DEFINED(__ram_trial) ? 0x40007000 : 0x40001000,
               LENGTH = DEFINED(__ram_trial) ? 0xee0 : 0x6ee0
    AUX (rw) : ORIGIN = 0x7fd00000, LENGTH = 8K
}
SECTIONS {
    .vectors ORIGIN(IMAGE) : { KEEP(*(.vectors)) } > IMAGE
    .revision ORIGIN(IMAGE) + 0x100 : { KEEP(*(.revision)) } > IMAGE
    .crp ORIGIN(IMAGE) + 0x1fc : { LONG(0xffffffff) } > IMAGE
    .text : {
        *(.text.startup) *(.text .text.*)
        *(.rodata .rodata.*)
    } > IMAGE
    .ARM.exidx : { *(.ARM.exidx*) } > IMAGE
    .data : ALIGN(4) {
        __data_start = .;
        *(.data .data.*)
        . = ALIGN(4);
        __data_end = .;
    } > RAM AT > IMAGE
    __data_load = LOADADDR(.data);
    .main_bss (NOLOAD) : ALIGN(4) {
        __main_start = .;
        *(.bss.inventory)
        . = ALIGN(4);
        __main_end = .;
    } > RAM
    .low_bss (NOLOAD) : ALIGN(4) {
        __low_start = .;
        *(.low_bss)
        . = ALIGN(4);
        __low_end = .;
    } > LOW
    .bss (NOLOAD) : ALIGN(4) {
        __bss_start = .;
        *(.bss .bss.* COMMON)
        . = ALIGN(4);
        __bss_end = .;
    } > AUX
    __stack_top = ORIGIN(RAM) + LENGTH(RAM);
    ASSERT(SIZEOF(.vectors) == 64, "bad ARM7 vector table")
    ASSERT(__main_end + 2048 <= __stack_top, "not enough stack")
    /DISCARD/ : { *(.ARM.extab*) *(.eh_frame*) }
}
