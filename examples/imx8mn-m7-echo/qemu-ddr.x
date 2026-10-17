/* SPDX-License-Identifier: BSD-3-Clause */
/*
 * QEMU imx8mp-evk (Cortex-M7 AMP series): the M7 shares the A53 physical
 * map and boots from the vector table at 0x80000000, so everything lives in
 * the DDR carve-out reserved for the M7 (m7@80000000 in the device tree).
 */
MEMORY
{
  CODE : ORIGIN = 0x80000000, LENGTH = 8M     /* vectors, code, rodata, .data image */
  DATA : ORIGIN = 0x80800000, LENGTH = 7M     /* data, bss, stack */
  RSC  : ORIGIN = 0x80F00000, LENGTH = 4K     /* resource table */
}

ENTRY(Reset)
_stack_top = ORIGIN(DATA) + LENGTH(DATA);

SECTIONS
{
  .vector_table ORIGIN(CODE) :
  {
    KEEP(*(.vector_table));
  } > CODE

  .text : ALIGN(4)
  {
    *(.text .text.*);
  } > CODE

  .rodata : ALIGN(4)
  {
    *(.rodata .rodata.*);
    . = ALIGN(4);
  } > CODE

  .data : ALIGN(4)
  {
    __sdata = .;
    *(.data .data.*);
    . = ALIGN(4);
    __edata = .;
  } > DATA AT > CODE
  __sidata = LOADADDR(.data);

  .bss (NOLOAD) : ALIGN(4)
  {
    __sbss = .;
    *(.bss .bss.*);
    *(COMMON);
    . = ALIGN(4);
    __ebss = .;
  } > DATA

  /*
   * The resource table is a loadable section of its own, outside .data, so
   * the reset code never copies over what Linux wrote into it. Linux finds
   * it by name.
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
