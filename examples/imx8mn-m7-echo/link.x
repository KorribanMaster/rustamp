/* SPDX-License-Identifier: BSD-3-Clause */
/*
 * i.MX 8M Nano Cortex-M7, as seen from the M7. Linux (imx_rproc) loads the
 * ELF into the TCMs through its own aliases (ITCM at 0x007E0000, DTCM at
 * 0x00800000 on the A53 side). The M7 boots from the vector table at 0.
 */
MEMORY
{
  ITCM : ORIGIN = 0x00000000, LENGTH = 128K   /* vectors, code, rodata, .data image */
  DTCM : ORIGIN = 0x20000000, LENGTH = 124K   /* data, bss, stack */
  RSC  : ORIGIN = 0x2001F000, LENGTH = 4K     /* resource table */
}

ENTRY(Reset)
_stack_top = ORIGIN(DTCM) + LENGTH(DTCM);

SECTIONS
{
  .vector_table ORIGIN(ITCM) :
  {
    KEEP(*(.vector_table));
  } > ITCM

  .text : ALIGN(4)
  {
    *(.text .text.*);
  } > ITCM

  .rodata : ALIGN(4)
  {
    *(.rodata .rodata.*);
    . = ALIGN(4);
  } > ITCM

  .data : ALIGN(4)
  {
    __sdata = .;
    *(.data .data.*);
    . = ALIGN(4);
    __edata = .;
  } > DTCM AT > ITCM
  __sidata = LOADADDR(.data);

  .bss (NOLOAD) : ALIGN(4)
  {
    __sbss = .;
    *(.bss .bss.*);
    *(COMMON);
    . = ALIGN(4);
    __ebss = .;
  } > DTCM

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
