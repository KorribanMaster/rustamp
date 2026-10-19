/* SPDX-License-Identifier: BSD-3-Clause */
/*
 * Zynq UltraScale+ RPU, R5_0 in split mode, as seen from the R5. Linux
 * (xlnx_r5_remoteproc) loads the ELF into the TCMs, which the R5 boots from
 * at 0, and into the rproc_0_fw_image carve-out in DDR. aarch32-rt's link.x
 * includes this file.
 */
MEMORY
{
  ATCM  : ORIGIN = 0x00000000, LENGTH = 64K   /* vectors, code, rodata, .data image */
  BTCM  : ORIGIN = 0x00020000, LENGTH = 64K   /* data, bss, stacks */
  RSC   : ORIGIN = 0x3ED00000, LENGTH = 4K    /* resource table (DDR) */
  TRACE : ORIGIN = 0x3ED01000, LENGTH = 4K    /* trace buffer (DDR), see board.rs */
}

REGION_ALIAS("VECTORS", ATCM);
REGION_ALIAS("CODE", ATCM);
REGION_ALIAS("DATA", BTCM);
REGION_ALIAS("STACKS", BTCM);

/* link.x defaults to 16 KiB per mode; BTCM is 64 KiB in all. */
_und_stack_size = 1K;
_svc_stack_size = 2K;
_abt_stack_size = 1K;
_hyp_stack_size = 0;
_irq_stack_size = 2K;
_fiq_stack_size = 64;
_sys_stack_size = 16K;

/*
 * The resource table and the trace buffer are sections of their own in DDR,
 * so the reset code neither copies nor zeroes them: Linux writes the table
 * before starting us, and finds both by address (the table also by name).
 * They go after .uninit: link.x takes __edata and __ebss from the location
 * after .data and .bss, so anything inserted there would be copied over or
 * zeroed.
 */
SECTIONS
{
  .resource_table : ALIGN(8)
  {
    KEEP(*(.resource_table));
  } > RSC

  .rsc_trace (NOLOAD) :
  {
    KEEP(*(.rsc_trace));
  } > TRACE
}
INSERT AFTER .uninit;
