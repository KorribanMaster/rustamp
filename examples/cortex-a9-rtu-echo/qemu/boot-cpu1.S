/* SPDX-License-Identifier: BSD-3-Clause */
/*
 * CPU1 entry for QEMU's xilinx-zynq-a9, loaded at 0xfffffe00 in on-chip
 * memory: the same wait loop the Zynq boot ROM / FSBL leaves CPU1 in on
 * real hardware. It waits for an entry address at 0xfffffff0 and jumps
 * there; Linux's a9-rproc driver ("release-addr" start method) writes the
 * address and signals SEV.
 */
	.arm
	.global _start
_start:
	ldr	r0, =0xfffffff0
1:	wfe
	ldr	r1, [r0]
	cmp	r1, #0
	beq	1b
	bx	r1
	.ltorg
