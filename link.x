ENTRY(_start)
MEMORY {
    /* Stay inside the sectors erased by the stock J1196J updater. */
    IMAGE (rx) : ORIGIN = DEFINED(__ram_trial) ? 0x40001000 : 0,
                 LENGTH = DEFINED(__ram_trial) ? 16K : 64K
    /* Leave the bottom 0x200 and top 0x120 bytes to the bootloader. */
    RAM (rw) : ORIGIN = DEFINED(__ram_trial) ? 0x40005000 : 0x40000200,
               LENGTH = DEFINED(__ram_trial) ? 0x2ee0 : 0x7ce0
}
SECTIONS {
    .vectors ORIGIN(IMAGE) : { KEEP(*(.vectors)) } > IMAGE
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
    .bss (NOLOAD) : ALIGN(4) {
        __bss_start = .;
        *(.bss .bss.* COMMON)
        . = ALIGN(4);
        __bss_end = .;
    } > RAM
    __stack_top = ORIGIN(RAM) + LENGTH(RAM);
    ASSERT(SIZEOF(.vectors) == 64, "bad ARM7 vector table")
    ASSERT(__bss_end + 2048 <= __stack_top, "not enough stack")
    /DISCARD/ : { *(.ARM.extab*) *(.eh_frame*) }
}
