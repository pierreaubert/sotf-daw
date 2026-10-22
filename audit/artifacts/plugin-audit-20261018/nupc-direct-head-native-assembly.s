	.globl	__RNvMs_NtNtCs3hdjNYYQN2G_15plugins_spatial4nupc11nupc_engineNtB4_10NupcEngine14process_sample
	.p2align	2
__RNvMs_NtNtCs3hdjNYYQN2G_15plugins_spatial4nupc11nupc_engineNtB4_10NupcEngine14process_sample:
	.cfi_startproc
	stp	d9, d8, [sp, #-96]!
	.cfi_def_cfa_offset 96
	stp	x26, x25, [sp, #16]
	stp	x24, x23, [sp, #32]
	stp	x22, x21, [sp, #48]
	stp	x20, x19, [sp, #64]
	stp	x29, x30, [sp, #80]
	add	x29, sp, #80
	.cfi_def_cfa w29, 16
	.cfi_offset w30, -8
	.cfi_offset w29, -16
	.cfi_offset w19, -24
	.cfi_offset w20, -32
	.cfi_offset w21, -40
	.cfi_offset w22, -48
	.cfi_offset w23, -56
	.cfi_offset w24, -64
	.cfi_offset w25, -72
	.cfi_offset w26, -80
	.cfi_offset b8, -88
	.cfi_offset b9, -96
	.cfi_remember_state
	mov.16b	v8, v0
	mov	x19, x0
	ldr	x8, [x0, #24]
	cmn	x8, #1
	b.eq	LBB23_4
	ldr	x20, [x19, #64]
	ldr	x1, [x19, #40]
	cmp	x20, x1
	b.hs	LBB23_9
	ldr	x21, [x19, #32]
	str	s8, [x21, x20, lsl #2]
	ldr	x8, [x19, #56]
	cmp	x20, x8
	b.hs	LBB23_8
	add	x25, x20, #1
	ldr	x9, [x19, #48]
	add	x22, x9, #16
	sub	x23, x1, x25
	lsl	x26, x25, #2
	sub	x24, x8, x25
	add	x1, x20, #1
	add	x3, x20, #1
	mov	x0, x22
	mov	x2, x21
	bl	__RNvNtNtCs3hdjNYYQN2G_15plugins_spatial4nupc16time_domain_head12reversed_dot
	mov.16b	v9, v0
	add	x0, x22, x26
	add	x2, x21, x26
	mov	x1, x24
	mov	x3, x23
	bl	__RNvNtNtCs3hdjNYYQN2G_15plugins_spatial4nupc16time_domain_head12reversed_dot
	ldr	x8, [x19, #72]
	cmp	x25, x8
	csinc	x8, xzr, x20, eq
	str	x8, [x19, #64]
	fadd	s0, s9, s0
	movi.2d	v1, #0000000000000000
	fadd	s9, s0, s1
	ldr	x8, [x19, #16]
	cbnz	x8, LBB23_5
	b	LBB23_7
LBB23_4:
	movi.2d	v9, #0000000000000000
	ldr	x8, [x19, #16]
	cbz	x8, LBB23_7
LBB23_5:
	ldr	x0, [x19, #8]
	lsl	x19, x8, #8
LBB23_6:
	add	x20, x0, #256
	mov.16b	v0, v8
	bl	__RNvMs_NtNtCs3hdjNYYQN2G_15plugins_spatial4nupc15partition_levelNtB4_14PartitionLevel11push_sample
	fadd	s9, s9, s0
	mov	x0, x20
	subs	x19, x19, #256
	b.ne	LBB23_6
LBB23_7:
	mov.16b	v0, v9
	.cfi_def_cfa wsp, 96
	ldp	x29, x30, [sp, #80]
	ldp	x20, x19, [sp, #64]
	ldp	x22, x21, [sp, #48]
	ldp	x24, x23, [sp, #32]
	ldp	x26, x25, [sp, #16]
	ldp	d9, d8, [sp], #96
	.cfi_def_cfa_offset 0
	.cfi_restore w30
	.cfi_restore w29
	.cfi_restore w19
	.cfi_restore w20
	.cfi_restore w21
	.cfi_restore w22
	.cfi_restore w23
	.cfi_restore w24
	.cfi_restore w25
	.cfi_restore w26
	.cfi_restore b8
	.cfi_restore b9
	ret
LBB23_8:
	.cfi_restore_state
Lloh4:
	adrp	x0, l_anon.b843fb4df9a8d749678d4427c273001d.3@PAGE
Lloh5:
	add	x0, x0, l_anon.b843fb4df9a8d749678d4427c273001d.3@PAGEOFF
Lloh6:
	adrp	x2, l_anon.b843fb4df9a8d749678d4427c273001d.8@PAGE
Lloh7:
	add	x2, x2, l_anon.b843fb4df9a8d749678d4427c273001d.8@PAGEOFF
	mov	w1, #19
	bl	__RNvNtCs3bJEMBXHbwl_4core9panicking9panic_fmt
LBB23_9:
Lloh8:
	adrp	x2, l_anon.b843fb4df9a8d749678d4427c273001d.7@PAGE
Lloh9:
	add	x2, x2, l_anon.b843fb4df9a8d749678d4427c273001d.7@PAGEOFF
	mov	x0, x20
	bl	__RNvNtCs3bJEMBXHbwl_4core9panicking18panic_bounds_check
	.loh AdrpAdd	Lloh6, Lloh7
	.loh AdrpAdd	Lloh4, Lloh5
	.loh AdrpAdd	Lloh8, Lloh9
	.cfi_endproc
