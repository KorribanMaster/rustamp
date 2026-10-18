/* SPDX-License-Identifier: BSD-3-Clause */
/*
 * Cortex-A9 RTU (CPU1) firmware layout. Linux's remoteproc ELF loader puts
 * the segments straight at their link addresses (da == pa); src/board.rs
 * documents the whole carve-out and must agree with these regions.
 */
MEMORY
{
  FW  : ORIGIN = 0x3E000000, LENGTH = 0xF00000   /* code, data, bss, stacks */
  RSC : ORIGIN = 0x3EF00000, LENGTH = 0xFF000    /* resource table (park page above) */
}

ENTRY(_start)

SECTIONS
{
  .text ORIGIN(FW) :
  {
    KEEP(*(.vectors));
    *(.text.boot);
    *(.text .text.*);
  } > FW

  .rodata : ALIGN(4)
  {
    *(.rodata .rodata.*);
  } > FW

  /* Loaded in place by Linux (no copy at boot). Holds the MMU table. */
  .data : ALIGN(16384)
  {
    *(.data.mmu_table);
    *(.data .data.*);
  } > FW

  .bss (NOLOAD) : ALIGN(8)
  {
    __sbss = .;
    *(.bss .bss.*);
    *(COMMON);
    . = ALIGN(8);
    __ebss = .;
  } > FW

  /* Not zeroed at boot: the reset code is already running on them. */
  .stacks (NOLOAD) : ALIGN(8)
  {
    . += 0x400;  __und_stack = .;
    . += 0x400;  __abt_stack = .;
    . += 0x400;  __svc_stack = .;
    . += 0x1000; __irq_stack = .;
    . += 0x10000; __sys_stack = .;
  } > FW

  /*
   * Its own uncached section: Linux updates the vdev status and vring
   * addresses in place while the RTU polls them.
   */
  .resource_table : ALIGN(8)
  {
    KEEP(*(.resource_table));
  } > RSC

  /DISCARD/ :
  {
    *(.ARM.exidx .ARM.exidx.* .ARM.extab.*);
  }
}
