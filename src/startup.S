.syntax unified
.cpu arm7tdmi
.arm

.section .vectors,"ax",%progbits
/* Every instruction loads the handler from the corresponding word at +32. */
.rept 5
    ldr pc, [pc, #24]
.endr
    .word (-(7 * 0xe59ff018)) & 0xffffffff
    ldr pc, [pc, #24]
    ldr pc, [pc, #24]
    .word _start
.rept 7
    .word fault_halt
.endr

.section .text.startup,"ax",%progbits
.global _start
.type _start,%function
_start:
    msr cpsr_c, #0xd3          /* Supervisor mode, IRQ and FIQ masked. */
    ldr sp, =__stack_top
    ldr r0, =__data_load
    ldr r1, =__data_start
    ldr r2, =__data_end
1:  cmp r1, r2
    ldrlo r3, [r0], #4
    strlo r3, [r1], #4
    blo 1b
    /* UM10139 Table 31: power the LPC2146 auxiliary SRAM before use. */
    ldr r0, =0xe01fc0c4
    ldr r1, [r0]
    orr r1, r1, #0x80000000
    str r1, [r0]
    mov r3, #0
    ldr r1, =__low_start
    ldr r2, =__low_end
3:  cmp r1, r2
    strlo r3, [r1], #4
    blo 3b
    ldr r1, =__bss_start
    ldr r2, =__bss_end
2:  cmp r1, r2
    strlo r3, [r1], #4
    blo 2b
    ldr r0, =firmware_main
    bx r0
.size _start, .-_start

/* Thumb-1 has no CPSR access. These leaves return to either instruction set. */
.global arm7_mask_interrupts
.type arm7_mask_interrupts,%function
arm7_mask_interrupts:
    mrs r0, cpsr
    orr r1, r0, #0xc0
    msr cpsr_c, r1
    bx lr
.size arm7_mask_interrupts, .-arm7_mask_interrupts

.global arm7_restore_interrupts
.type arm7_restore_interrupts,%function
arm7_restore_interrupts:
    msr cpsr_c, r0
    bx lr
.size arm7_restore_interrupts, .-arm7_restore_interrupts

.global rom_iap
.type rom_iap,%function
rom_iap:
    /* ARMv4T interworking: r0/r1 point to command/result tables. */
    adr lr, fault_halt
    ldr r2, =0x7ffffff1
    bx r2
.size rom_iap, .-rom_iap

fault_halt:
    b fault_halt
    .ltorg
