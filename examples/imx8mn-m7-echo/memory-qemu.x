/* SPDX-License-Identifier: BSD-3-Clause */
/*
 * QEMU imx8mp-evk (Cortex-M7 AMP series): the M7 shares the A53 physical
 * map and boots from the vector table at 0x80000000, so everything lives in
 * the DDR carve-out reserved for the M7 (m7@80000000 in the device tree).
 *
 * cortex-m-rt's link.x includes this file as memory.x (see build.rs).
 */
MEMORY
{
  FLASH : ORIGIN = 0x80000000, LENGTH = 8M    /* vectors, code, rodata, .data image */
  RAM   : ORIGIN = 0x80800000, LENGTH = 7M    /* data, bss, stack */
  RSC   : ORIGIN = 0x80F00000, LENGTH = 4K    /* resource table */
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
