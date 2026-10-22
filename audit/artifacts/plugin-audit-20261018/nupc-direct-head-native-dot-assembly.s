__RNvNtNtCs3hdjNYYQN2G_15plugins_spatial4nupc16time_domain_head12reversed_dot:
	.cfi_startproc
	sub	sp, sp, #224
	.cfi_def_cfa_offset 224
	stp	d11, d10, [sp, #112]
	stp	d9, d8, [sp, #128]
	stp	x26, x25, [sp, #144]
	stp	x24, x23, [sp, #160]
	stp	x22, x21, [sp, #176]
	stp	x20, x19, [sp, #192]
	stp	x29, x30, [sp, #208]
	add	x29, sp, #208
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
	.cfi_offset b10, -104
	.cfi_offset b11, -112
	.cfi_remember_state
	mov	x20, x3
	mov	x19, x2
	mov	x22, x1
	mov	x21, x0
	lsr	x8, x1, #2
	add	x1, x0, x8, lsl #4
	and	x8, x3, #0x3
	add	x9, x2, x8, lsl #2
	and	x10, x3, #0x1ffffffffffffffc
	stp	x9, x10, [sp, #72]
	stp	x2, x8, [sp, #88]
	mov	w8, #4
	str	x8, [sp, #104]
	mov	x24, sp
	mov	x8, sp
	add	x2, sp, #72
	bl	__RNvXs3_NtNtNtCs3bJEMBXHbwl_4core4iter8adapters3zipINtB5_3ZipINtNtNtBb_5slice4iter4IterAfj4_EINtBZ_12RChunksExactfEEINtB5_7ZipImplBW_B1s_E3newCs3hdjNYYQN2G_15plugins_spatial
	ldp	x23, x8, [sp, #56]
	movi.2d	v0, #0000000000000000
	cmp	x23, x8
	b.hs	LBB28_5
	movi.2d	v8, #0000000000000000
	movi.2d	v9, #0000000000000000
	movi.2d	v10, #0000000000000000
	movi.2d	v11, #0000000000000000
LBB28_2:
	add	x8, x23, #1
	str	x8, [sp, #56]
	ldr	x25, [sp]
	add	x0, x24, #16
	mov	x1, x23
	bl	__RNvXs2b_NtNtCs3bJEMBXHbwl_4core5slice4iterINtB6_12RChunksExactfENtNtNtNtBa_4iter6traits8iterator8Iterator24___iterator_get_uncheckedCs3hdjNYYQN2G_15plugins_spatial
	cmp	x1, #3
	b.ls	LBB28_11
	add	x8, x25, x23, lsl #4
	ldp	s0, s1, [x8]
	ldp	s3, s2, [x0, #8]
	fmul	s0, s0, s2
	fadd	s11, s11, s0
	fmul	s0, s1, s3
	fadd	s10, s10, s0
	ldp	s0, s1, [x8, #8]
	ldp	s3, s2, [x0]
	fmul	s0, s0, s2
	fadd	s9, s9, s0
	fmul	s0, s1, s3
	fadd	s8, s8, s0
	ldp	x23, x8, [sp, #56]
	cmp	x23, x8
	b.lo	LBB28_2
	fadd	s0, s11, s10
	fadd	s0, s0, s9
	fadd	s0, s0, s8
LBB28_5:
	and	x1, x22, #0x3
	cmp	x1, x20
	b.hi	LBB28_10
	cbz	x1, LBB28_9
	and	x8, x22, #0x1ffffffffffffffc
	add	x8, x21, x8, lsl #2
	lsl	x9, x1, #2
	sub	x9, x9, #4
LBB28_8:
	ldr	s1, [x8], #4
	mov	x10, x9
	ldr	s2, [x19, x9]
	fmul	s1, s1, s2
	fadd	s0, s0, s1
	sub	x9, x9, #4
	cbnz	x10, LBB28_8
LBB28_9:
	.cfi_def_cfa wsp, 224
	ldp	x29, x30, [sp, #208]
	ldp	x20, x19, [sp, #192]
	ldp	x22, x21, [sp, #176]
	ldp	x24, x23, [sp, #160]
	ldp	x26, x25, [sp, #144]
	ldp	d9, d8, [sp, #128]
	ldp	d11, d10, [sp, #112]
	add	sp, sp, #224
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
	.cfi_restore b10
	.cfi_restore b11
	ret
LBB28_10:
	.cfi_restore_state
Lloh14:
	adrp	x3, l_anon.b843fb4df9a8d749678d4427c273001d.12@PAGE
Lloh15:
	add	x3, x3, l_anon.b843fb4df9a8d749678d4427c273001d.12@PAGEOFF
	mov	x0, #0
	mov	x2, x20
	bl	__RNvNtNtCs3bJEMBXHbwl_4core5slice5index16slice_index_fail
LBB28_11:
Lloh16:
	adrp	x2, l_anon.b843fb4df9a8d749678d4427c273001d.13@PAGE
Lloh17:
	add	x2, x2, l_anon.b843fb4df9a8d749678d4427c273001d.13@PAGEOFF
	mov	w0, #3
	bl	__RNvNtCs3bJEMBXHbwl_4core9panicking18panic_bounds_check
	.loh AdrpAdd	Lloh14, Lloh15
	.loh AdrpAdd	Lloh16, Lloh17
	.cfi_endproc
