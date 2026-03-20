use super::*;

impl<'a> Emitter<'a> {
    /// Emit an inline load from a Buffer region at a constant address.
    /// The base pointer is baked as an immediate -- zero overhead.
    pub(super) fn emit_buffer_load(&mut self, base: *mut u32, idx: u32) -> Value {
        let base_val = self.builder.ins().iconst(self.ptr_ty, base as i64);
        let byte_off = (idx as i64) * 4;
        self.builder
            .ins()
            .load(types::I32, MemFlags::trusted(), base_val, byte_off as i32)
    }

    /// Emit an inline store to a Buffer region at a constant address.
    pub(super) fn emit_buffer_store(&mut self, base: *mut u32, idx: u32, val: Value) {
        let base_val = self.builder.ins().iconst(self.ptr_ty, base as i64);
        let byte_off = (idx as i64) * 4;
        self.builder
            .ins()
            .store(MemFlags::trusted(), val, base_val, byte_off as i32);
    }

    /// Compute the native address of `base[(addr - adj) as usize]` where
    /// `adj = start.wrapping_sub(offset)`.
    fn emit_buffer_elem_addr(
        &mut self,
        base: *mut u32,
        start: u32,
        offset: u32,
        addr: Value,
    ) -> Value {
        let base_val = self.builder.ins().iconst(self.ptr_ty, base as i64);
        let adj = start.wrapping_sub(offset);
        let index = if adj == 0 {
            addr
        } else {
            self.builder.ins().iadd_imm(addr, -(adj as i64))
        };
        let byte_off = self.builder.ins().ishl_imm(index, 2);
        let byte_off_ext = if self.ptr_ty == types::I64 {
            self.builder.ins().uextend(types::I64, byte_off)
        } else {
            byte_off
        };
        self.builder.ins().iadd(base_val, byte_off_ext)
    }

    /// Emit an inline load from a Buffer region at a dynamic address.
    pub(super) fn emit_buffer_load_dyn(
        &mut self,
        base: *mut u32,
        start: u32,
        offset: u32,
        addr: Value,
    ) -> Value {
        let elem_addr = self.emit_buffer_elem_addr(base, start, offset, addr);
        self.builder
            .ins()
            .load(types::I32, MemFlags::trusted(), elem_addr, 0)
    }

    /// Emit an inline store to a Buffer region at a dynamic address.
    pub(super) fn emit_buffer_store_dyn(
        &mut self,
        base: *mut u32,
        start: u32,
        offset: u32,
        addr: Value,
        val: Value,
    ) {
        let elem_addr = self.emit_buffer_elem_addr(base, start, offset, addr);
        self.builder
            .ins()
            .store(MemFlags::trusted(), val, elem_addr, 0);
    }

    /// Emit an indirect call to a Callback region's read function.
    /// Does NOT flush/reload promoted registers -- caller is responsible.
    pub(super) fn emit_callback_read_dyn(
        &mut self,
        opaque: *mut std::ffi::c_void,
        read_fn: unsafe extern "C" fn(*mut std::ffi::c_void, u32) -> u32,
        addr: Value,
    ) -> Value {
        let fn_val = self
            .builder
            .ins()
            .iconst(self.ptr_ty, read_fn as usize as i64);
        let opaque_val = self
            .builder
            .ins()
            .iconst(self.ptr_ty, opaque as usize as i64);
        let mut sig = Signature::new(HOST_CALL_CONV);
        sig.params.push(AbiParam::new(self.ptr_ty)); // opaque
        sig.params.push(AbiParam::new(types::I32)); // address
        sig.returns.push(AbiParam::new(types::I32)); // return
        let sig_ref = self.builder.import_signature(sig);
        let call = self
            .builder
            .ins()
            .call_indirect(sig_ref, fn_val, &[opaque_val, addr]);
        self.builder.inst_results(call)[0]
    }

    /// Emit an indirect call to a Callback region's write function.
    /// Does NOT flush/reload promoted registers -- caller is responsible.
    pub(super) fn emit_callback_write_dyn(
        &mut self,
        opaque: *mut std::ffi::c_void,
        write_fn: unsafe extern "C" fn(*mut std::ffi::c_void, u32, u32),
        addr: Value,
        val: Value,
    ) {
        let fn_val = self
            .builder
            .ins()
            .iconst(self.ptr_ty, write_fn as usize as i64);
        let opaque_val = self
            .builder
            .ins()
            .iconst(self.ptr_ty, opaque as usize as i64);
        let mut sig = Signature::new(HOST_CALL_CONV);
        sig.params.push(AbiParam::new(self.ptr_ty)); // opaque
        sig.params.push(AbiParam::new(types::I32)); // address
        sig.params.push(AbiParam::new(types::I32)); // value
        let sig_ref = self.builder.import_signature(sig);
        self.builder
            .ins()
            .call_indirect(sig_ref, fn_val, &[opaque_val, addr, val]);
    }

    /// Read from a memory space at a constant address (compile-time map lookup).
    pub(super) fn read_mem(&mut self, space: MemSpace, addr: u32) -> Value {
        let raw = if let Some(region) = self.map.lookup(space, addr) {
            match region.kind {
                RegionKind::Buffer { base, offset } => {
                    self.emit_buffer_load(base, addr - region.start + offset)
                }
                RegionKind::Callback {
                    opaque, read_fn, ..
                } => {
                    self.flush_promoted();
                    let addr_val = self.builder.ins().iconst(types::I32, addr as i64);
                    let result = self.emit_callback_read_dyn(opaque, read_fn, addr_val);
                    self.invalidate_promoted();
                    result
                }
            }
        } else {
            self.builder.ins().iconst(types::I32, 0)
        };
        self.mask24(raw)
    }

    /// Write to a memory space at a constant address (compile-time map lookup).
    pub(super) fn write_mem(&mut self, space: MemSpace, addr: u32, val: Value) {
        if let Some(region) = self.map.lookup(space, addr) {
            let masked = self.mask24(val);
            match region.kind {
                RegionKind::Buffer { base, offset } => {
                    self.emit_buffer_store(base, addr - region.start + offset, masked);
                }
                RegionKind::Callback {
                    opaque, write_fn, ..
                } => {
                    self.flush_promoted();
                    let addr_val = self.builder.ins().iconst(types::I32, addr as i64);
                    self.emit_callback_write_dyn(opaque, write_fn, addr_val, masked);
                    self.invalidate_promoted();
                }
            }
        }
    }

    /// Read memory at a dynamic address via inline region dispatch.
    /// Generates a branch tree that checks each region at JIT compile time,
    /// inlining buffer loads and baking callback pointers as constants.
    /// Buffer regions skip flush/reload entirely.
    pub(super) fn read_mem_dyn(&mut self, space: MemSpace, addr: Value) -> Value {
        let regions = self.map.regions(space).to_vec();
        if regions.is_empty() {
            return self.builder.ins().iconst(types::I32, 0);
        }

        let has_callbacks = regions
            .iter()
            .any(|r| matches!(r.kind, RegionKind::Callback { .. }));
        if has_callbacks {
            self.flush_promoted();
        }

        let n = regions.len();
        let check_blocks: Vec<_> = (0..n).map(|_| self.builder.create_block()).collect();
        let body_blocks: Vec<_> = (0..n).map(|_| self.builder.create_block()).collect();
        let fallthrough_block = self.builder.create_block();
        let merge_block = self.builder.create_block();
        self.builder.append_block_param(merge_block, types::I32);

        self.builder.ins().jump(check_blocks[0], &[]);

        for i in 0..n {
            let region = &regions[i];
            let next = if i + 1 < n {
                check_blocks[i + 1]
            } else {
                fallthrough_block
            };

            // Check block: range test
            self.builder.switch_to_block(check_blocks[i]);
            self.builder.seal_block(check_blocks[i]);
            if region.start == 0 {
                let lt_end =
                    self.builder
                        .ins()
                        .icmp_imm(IntCC::UnsignedLessThan, addr, region.end as i64);
                self.builder
                    .ins()
                    .brif(lt_end, body_blocks[i], &[], next, &[]);
            } else {
                let ge_start = self.builder.ins().icmp_imm(
                    IntCC::UnsignedGreaterThanOrEqual,
                    addr,
                    region.start as i64,
                );
                let lt_end =
                    self.builder
                        .ins()
                        .icmp_imm(IntCC::UnsignedLessThan, addr, region.end as i64);
                let in_range = self.builder.ins().band(ge_start, lt_end);
                self.builder
                    .ins()
                    .brif(in_range, body_blocks[i], &[], next, &[]);
            }

            // Body block: dispatch by region kind
            self.builder.switch_to_block(body_blocks[i]);
            self.builder.seal_block(body_blocks[i]);
            match region.kind {
                RegionKind::Buffer { base, offset } => {
                    let result = self.emit_buffer_load_dyn(base, region.start, offset, addr);
                    self.builder
                        .ins()
                        .jump(merge_block, &[BlockArg::Value(result)]);
                }
                RegionKind::Callback {
                    opaque, read_fn, ..
                } => {
                    let result = self.emit_callback_read_dyn(opaque, read_fn, addr);
                    self.builder
                        .ins()
                        .jump(merge_block, &[BlockArg::Value(result)]);
                }
            }
        }

        self.builder.switch_to_block(fallthrough_block);
        self.builder.seal_block(fallthrough_block);
        let zero = self.builder.ins().iconst(types::I32, 0);
        self.builder
            .ins()
            .jump(merge_block, &[BlockArg::Value(zero)]);

        self.builder.switch_to_block(merge_block);
        self.builder.seal_block(merge_block);
        if has_callbacks {
            self.invalidate_promoted();
        }
        let raw = self.builder.block_params(merge_block)[0];
        self.mask24(raw)
    }

    /// Write memory at a dynamic address via inline region dispatch.
    /// P-space writes use the runtime helper for dirty bitmap tracking.
    /// X/Y writes are fully inlined.
    pub(super) fn write_mem_dyn(&mut self, space: MemSpace, addr: Value, val: Value) {
        if space == MemSpace::P {
            // P-space writes need dirty tracking; keep using runtime helper
            self.flush_promoted();
            let fn_ptr = self
                .builder
                .ins()
                .iconst(self.ptr_ty, jit_write_mem as *const () as usize as i64);
            let space_val = self
                .builder
                .ins()
                .iconst(types::I32, MemSpace::P as u32 as i64);
            let mut sig = Signature::new(HOST_CALL_CONV);
            sig.params.push(AbiParam::new(self.ptr_ty));
            sig.params.push(AbiParam::new(types::I32));
            sig.params.push(AbiParam::new(types::I32));
            sig.params.push(AbiParam::new(types::I32));
            let sig_ref = self.builder.import_signature(sig);
            self.builder.ins().call_indirect(
                sig_ref,
                fn_ptr,
                &[self.state_ptr, space_val, addr, val],
            );
            self.invalidate_promoted();
            return;
        }

        let regions = self.map.regions(space).to_vec();
        if regions.is_empty() {
            return;
        }

        // Mask to 24 bits
        let mask = self.builder.ins().iconst(types::I32, 0x00FF_FFFF);
        let masked = self.builder.ins().band(val, mask);

        let has_callbacks = regions
            .iter()
            .any(|r| matches!(r.kind, RegionKind::Callback { .. }));
        if has_callbacks {
            self.flush_promoted();
        }

        let n = regions.len();
        let check_blocks: Vec<_> = (0..n).map(|_| self.builder.create_block()).collect();
        let body_blocks: Vec<_> = (0..n).map(|_| self.builder.create_block()).collect();
        let fallthrough_block = self.builder.create_block();
        let merge_block = self.builder.create_block();

        self.builder.ins().jump(check_blocks[0], &[]);

        for i in 0..n {
            let region = &regions[i];
            let next = if i + 1 < n {
                check_blocks[i + 1]
            } else {
                fallthrough_block
            };

            self.builder.switch_to_block(check_blocks[i]);
            self.builder.seal_block(check_blocks[i]);
            if region.start == 0 {
                let lt_end =
                    self.builder
                        .ins()
                        .icmp_imm(IntCC::UnsignedLessThan, addr, region.end as i64);
                self.builder
                    .ins()
                    .brif(lt_end, body_blocks[i], &[], next, &[]);
            } else {
                let ge_start = self.builder.ins().icmp_imm(
                    IntCC::UnsignedGreaterThanOrEqual,
                    addr,
                    region.start as i64,
                );
                let lt_end =
                    self.builder
                        .ins()
                        .icmp_imm(IntCC::UnsignedLessThan, addr, region.end as i64);
                let in_range = self.builder.ins().band(ge_start, lt_end);
                self.builder
                    .ins()
                    .brif(in_range, body_blocks[i], &[], next, &[]);
            }

            self.builder.switch_to_block(body_blocks[i]);
            self.builder.seal_block(body_blocks[i]);
            match region.kind {
                RegionKind::Buffer { base, offset } => {
                    self.emit_buffer_store_dyn(base, region.start, offset, addr, masked);
                    self.builder.ins().jump(merge_block, &[]);
                }
                RegionKind::Callback {
                    opaque, write_fn, ..
                } => {
                    self.emit_callback_write_dyn(opaque, write_fn, addr, masked);
                    self.builder.ins().jump(merge_block, &[]);
                }
            }
        }

        self.builder.switch_to_block(fallthrough_block);
        self.builder.seal_block(fallthrough_block);
        self.builder.ins().jump(merge_block, &[]);

        self.builder.switch_to_block(merge_block);
        self.builder.seal_block(merge_block);
        if has_callbacks {
            self.invalidate_promoted();
        }
    }
}
