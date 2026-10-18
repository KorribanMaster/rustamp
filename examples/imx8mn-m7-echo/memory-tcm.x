/* SPDX-License-Identifier: BSD-3-Clause */
/*
 * i.MX 8M Nano Cortex-M7, as seen from the M7. Linux (imx_rproc) loads the
 * ELF into the TCMs through its own aliases (ITCM at 0x007E0000, DTCM at
 * 0x00800000 on the A53 side). The M7 boots from the vector table at 0.
 *
 * cortex-m-rt's link.x includes this file as memory.x (see build.rs).
 */
MEMORY
{
  FLASH : ORIGIN = 0x00000000, LENGTH = 128K  /* ITCM: vectors, code, rodata, .data image */
  RAM   : ORIGIN = 0x20000000, LENGTH = 124K  /* DTCM: data, bss, stack */
  RSC   : ORIGIN = 0x2001F000, LENGTH = 4K    /* DTCM: resource table */
}

/*
 * The resource table is a loadable section of its own, in its own region,
 * so the reset code neither copies nor zeroes it: Linux writes into it
 * before starting us, and finds it by name.
 *
 * Where it goes in link.x matters. After .data, it would be copied over
 * from flash; after .bss, zeroed (link.x takes __ebss from the location
 * after .bss). So it goes after .uninit, and since link.x takes the stack
 * end and heap start from the location there, those are pinned to the end
 * of RAM's static data instead.
 */
SECTIONS
{
  .resource_table : ALIGN(8)
  {
    KEEP(*(.resource_table));
  } > RSC
}
INSERT AFTER .uninit;

_stack_end = __euninit;
__sheap = __euninit;
