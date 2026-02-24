use super::*;

impl<'a> Emitter<'a> {
    /// Load SR and clear the given flag bits. Returns the cleared SR value.
    /// `flags` is a bitmask, e.g. `(1u32 << sr::N) | (1u32 << sr::Z)`.
    pub(super) fn clear_sr_flags(&mut self, flags: u32) -> Value {
        let sr = self.load_reg(reg::SR);
        let mask = self.builder.ins().iconst(types::I32, !flags as i64);
        self.builder.ins().band(sr, mask)
    }

    /// Update CCR flags E, U, N, Z from a 56-bit accumulator result.
    ///
    /// Computes flags based on the scaling mode (S1:S0 bits in SR).
    pub(super) fn update_nz(&mut self, acc_val: Value) {
        let sr_val = self.load_reg(reg::SR);
        let clear_mask = !((1u32 << sr::E) | (1u32 << sr::U) | (1u32 << sr::N) | (1u32 << sr::Z));
        let clear = self.builder.ins().iconst(types::I32, clear_mask as i64);
        let sr_c = self.builder.ins().band(sr_val, clear);

        // Extract accumulator parts (reg2/LSP not needed; Z uses full acc_val):
        //   reg0 = bits 55:48 (extension byte, 8 bits)
        //   reg1 = bits 47:24 (MSP, 24 bits)
        let c24 = self.builder.ins().iconst(types::I32, 24);
        let c48 = self.builder.ins().iconst(types::I32, 48);
        let mask24_i64 = self.builder.ins().iconst(types::I64, 0xFFFFFF);
        let mask8_i64 = self.builder.ins().iconst(types::I64, 0xFF);

        let reg1_64 = {
            let shifted = self.builder.ins().ushr(acc_val, c24);
            self.builder.ins().band(shifted, mask24_i64)
        };
        let reg0_64 = {
            let shifted = self.builder.ins().ushr(acc_val, c48);
            self.builder.ins().band(shifted, mask8_i64)
        };

        let reg0 = self.builder.ins().ireduce(types::I32, reg0_64);
        let reg1 = self.builder.ins().ireduce(types::I32, reg1_64);

        // Extract scaling mode: (SR >> S0) & 3
        let s0_shift = self.builder.ins().iconst(types::I32, sr::S0 as i64);
        let scaling = self.builder.ins().ushr(sr_val, s0_shift);
        let three = self.builder.ins().iconst(types::I32, 3);
        let scaling = self.builder.ins().band(scaling, three);

        // Compute E and U based on scaling mode
        // We implement scaling=0 (most common), scaling=1, scaling=2.
        // For scaling=3, E and U are not modified (cleared above).

        let zero32 = self.builder.ins().iconst(types::I32, 0);
        let one32 = self.builder.ins().iconst(types::I32, 1);

        // --- Scaling 0 (no scaling) ---
        // E: value_e = (reg0 << 1) | (reg1 >> 23)
        //    E = (value_e != 0 && value_e != 0x1FF)
        let reg0_shl1 = self.builder.ins().ishl(reg0, one32);
        let c23 = self.builder.ins().iconst(types::I32, 23);
        let reg1_shr23 = self.builder.ins().ushr(reg1, c23);
        let value_e0 = self.builder.ins().bor(reg0_shl1, reg1_shr23);
        let mask_9 = self.builder.ins().iconst(types::I32, 0x1FF);
        let value_e0 = self.builder.ins().band(value_e0, mask_9);
        let e0_nz = self.builder.ins().icmp(IntCC::NotEqual, value_e0, zero32);
        let e0_nff = self.builder.ins().icmp(IntCC::NotEqual, value_e0, mask_9);
        let e0_set = self.builder.ins().band(e0_nz, e0_nff);

        // U: (reg1 & 0xC00000) == 0 || (reg1 & 0xC00000) == 0xC00000
        let mask_c0 = self.builder.ins().iconst(types::I32, 0xC00000u32 as i64);
        let u0_bits = self.builder.ins().band(reg1, mask_c0);
        let u0_zero = self.builder.ins().icmp(IntCC::Equal, u0_bits, zero32);
        let u0_full = self.builder.ins().icmp(IntCC::Equal, u0_bits, mask_c0);
        let u0_set = self.builder.ins().bor(u0_zero, u0_full);

        // --- Scaling 1 (scale up) ---
        // E: (reg0 != 0 && reg0 != 0xFF)
        let mask_ff = self.builder.ins().iconst(types::I32, 0xFF);
        let e1_nz = self.builder.ins().icmp(IntCC::NotEqual, reg0, zero32);
        let e1_nff = self.builder.ins().icmp(IntCC::NotEqual, reg0, mask_ff);
        let e1_set = self.builder.ins().band(e1_nz, e1_nff);

        // U: ((reg0 << 1) | (reg1 >> 23)) & 3  == 0 or == 3
        let u1_val = self.builder.ins().band(value_e0, three);
        let u1_zero = self.builder.ins().icmp(IntCC::Equal, u1_val, zero32);
        let u1_full = self.builder.ins().icmp(IntCC::Equal, u1_val, three);
        let u1_set = self.builder.ins().bor(u1_zero, u1_full);

        // --- Scaling 2 (scale down) ---
        // E: value_e = (reg0 << 2) | (reg1 >> 22)
        //    E = (value_e != 0 && value_e != 0x3FF)
        let two = self.builder.ins().iconst(types::I32, 2);
        let reg0_shl2 = self.builder.ins().ishl(reg0, two);
        let c22 = self.builder.ins().iconst(types::I32, 22);
        let reg1_shr22 = self.builder.ins().ushr(reg1, c22);
        let value_e2 = self.builder.ins().bor(reg0_shl2, reg1_shr22);
        let mask_10 = self.builder.ins().iconst(types::I32, 0x3FF);
        let value_e2 = self.builder.ins().band(value_e2, mask_10);
        let e2_nz = self.builder.ins().icmp(IntCC::NotEqual, value_e2, zero32);
        let e2_nff = self.builder.ins().icmp(IntCC::NotEqual, value_e2, mask_10);
        let e2_set = self.builder.ins().band(e2_nz, e2_nff);

        // U: (reg1 & 0x600000) == 0 || (reg1 & 0x600000) == 0x600000
        let mask_60 = self.builder.ins().iconst(types::I32, 0x600000u32 as i64);
        let u2_bits = self.builder.ins().band(reg1, mask_60);
        let u2_zero = self.builder.ins().icmp(IntCC::Equal, u2_bits, zero32);
        let u2_full = self.builder.ins().icmp(IntCC::Equal, u2_bits, mask_60);
        let u2_set = self.builder.ins().bor(u2_zero, u2_full);

        // --- Select E and U based on scaling mode ---
        let s_is_0 = self.builder.ins().icmp(IntCC::Equal, scaling, zero32);
        let s_is_1 = self.builder.ins().icmp(IntCC::Equal, scaling, one32);
        let s_is_2 = self.builder.ins().icmp(IntCC::Equal, scaling, two);

        let zero8 = self.builder.ins().iconst(types::I8, 0);

        // E = (s==0 ? e0 : s==1 ? e1 : s==2 ? e2 : 0)
        let e_sel = self.builder.ins().select(s_is_2, e2_set, zero8);
        let e_sel = self.builder.ins().select(s_is_1, e1_set, e_sel);
        let e_sel = self.builder.ins().select(s_is_0, e0_set, e_sel);

        // U = (s==0 ? u0 : s==1 ? u1 : s==2 ? u2 : 0)
        let u_sel = self.builder.ins().select(s_is_2, u2_set, zero8);
        let u_sel = self.builder.ins().select(s_is_1, u1_set, u_sel);
        let u_sel = self.builder.ins().select(s_is_0, u0_set, u_sel);

        // Convert boolean results to SR bit positions
        let e32 = self.builder.ins().uextend(types::I32, e_sel);
        let e_bit = self.shift_to_bit(e32, sr::E);

        let u32_v = self.builder.ins().uextend(types::I32, u_sel);
        let u_bit = self.shift_to_bit(u32_v, sr::U);

        // N = bit 55
        let c55 = self.builder.ins().iconst(types::I32, 55);
        let n64 = self.builder.ins().ushr(acc_val, c55);
        let n32 = self.builder.ins().ireduce(types::I32, n64);
        let n_bit = self.shift_to_bit(n32, sr::N);

        // Z = (acc == 0)
        let zero64 = self.builder.ins().iconst(types::I64, 0);
        let is_z = self.builder.ins().icmp(IntCC::Equal, acc_val, zero64);
        let z32 = self.builder.ins().uextend(types::I32, is_z);
        let z_bit = self.shift_to_bit(z32, sr::Z);

        // Combine: SR = (SR & ~EUNZ) | E | U | N | Z
        let sr_new = self.builder.ins().bor(sr_c, e_bit);
        let sr_new = self.builder.ins().bor(sr_new, u_bit);
        let sr_new = self.builder.ins().bor(sr_new, n_bit);
        let sr_new = self.builder.ins().bor(sr_new, z_bit);
        self.store_reg(reg::SR, sr_new);
    }

    /// Update CCR flags for logical operations (AND, OR, EOR, NOT).
    ///
    /// Logical ops only clear N/Z/V and set N from bit 23 of the 24-bit
    /// result, Z if result == 0. E, U, C, L are left untouched.
    pub(super) fn update_nzv_logical(&mut self, result24: Value) {
        // Clear N, Z, V
        let sr_new = self.clear_sr_flags((1u32 << sr::N) | (1u32 << sr::Z) | (1u32 << sr::V));

        // N = (result >> 23) & 1
        let c23 = self.builder.ins().iconst(types::I32, 23);
        let n_raw = self.builder.ins().ushr(result24, c23);
        let one = self.builder.ins().iconst(types::I32, 1);
        let n_raw = self.builder.ins().band(n_raw, one);
        let n_bit = self.shift_to_bit(n_raw, sr::N);
        let sr_new = self.builder.ins().bor(sr_new, n_bit);

        // Z = (result == 0)
        let zero = self.builder.ins().iconst(types::I32, 0);
        let is_zero = self.builder.ins().icmp(IntCC::Equal, result24, zero);
        let z_val = self.builder.ins().uextend(types::I32, is_zero);
        let z_bit = self.shift_to_bit(z_val, sr::Z);
        let sr_new = self.builder.ins().bor(sr_new, z_bit);

        self.store_reg(reg::SR, sr_new);
    }

    /// Shift `overflow` (0/1 i32) to V and L bit positions and OR into `sr`.
    pub(super) fn or_vl(&mut self, sr: Value, overflow: Value) -> Value {
        let v_bit = self.shift_to_bit(overflow, sr::V);
        let l_bit = self.shift_to_bit(overflow, sr::L);
        let sr = self.builder.ins().bor(sr, v_bit);
        self.builder.ins().bor(sr, l_bit)
    }

    /// Update V and L from an overflow bit. Clears V, sets V and L (sticky).
    /// Does NOT modify C.
    pub(super) fn set_vl_overflow(&mut self, overflow: Value) {
        let sr_new = self.clear_sr_flags(1u32 << sr::V);
        let sr_new = self.or_vl(sr_new, overflow);
        self.store_reg(reg::SR, sr_new);
    }

    /// Arithmetic Saturation Mode (SM, SR bit 20) - value clamping.
    /// When SM=1, clamps a 56-bit accumulator result to 48 bits.
    /// Checks bits 55, 48, 47: if not all the same, clamps to max-pos or max-neg.
    /// Stores the `needs_sat` flag in `sm_needs_sat_var` for deferred V/L update.
    /// Returns the (possibly clamped) value. Caller MUST call `emit_sm_vl_deferred()`
    /// AFTER the instruction's normal flag computation.
    pub(super) fn emit_saturate_sm(&mut self, result: Value) -> Value {
        let sr = self.load_reg(reg::SR);
        // Extract SM bit (bit 20) from SR (i32)
        let sm_shift = self.builder.ins().iconst(types::I32, sr::SM as i64);
        let sm = self.builder.ins().ushr(sr, sm_shift);
        let one32 = self.builder.ins().iconst(types::I32, 1);
        let sm = self.builder.ins().band(sm, one32);

        let b55 = self.extract_bit_i64(result, 55);
        let b48 = self.extract_bit_i64(result, 48);
        let b47 = self.extract_bit_i64(result, 47);

        // needs_sat = SM & ((b55 ^ b48) | (b48 ^ b47))
        let xor_55_48 = self.builder.ins().bxor(b55, b48);
        let xor_48_47 = self.builder.ins().bxor(b48, b47);
        let mismatch = self.builder.ins().bor(xor_55_48, xor_48_47);
        let needs_sat = self.builder.ins().band(sm, mismatch);

        // Store needs_sat for deferred V/L update
        self.builder.def_var(self.sm_needs_sat_var, needs_sat);

        // Saturated value: bit 55 = 0 -> max positive, 1 -> max negative
        let max_pos = self
            .builder
            .ins()
            .iconst(types::I64, 0x0000_7FFF_FFFF_FFFF_i64);
        let max_neg = self
            .builder
            .ins()
            .iconst(types::I64, 0x00FF_8000_0000_0000_u64 as i64);
        let zero32 = self.builder.ins().iconst(types::I32, 0);
        let sign_nz = self.builder.ins().icmp(IntCC::NotEqual, b55, zero32);
        let saturated = self.builder.ins().select(sign_nz, max_neg, max_pos);

        // Select between original and saturated based on needs_sat
        let sat_nz = self.builder.ins().icmp(IntCC::NotEqual, needs_sat, zero32);
        self.builder.ins().select(sat_nz, saturated, result)
    }

    /// Apply deferred V/L flags from a previous `emit_saturate_sm` call.
    /// Call AFTER the instruction's normal flag computation (update_nz, update_vcl, etc.).
    pub(super) fn emit_sm_vl_deferred(&mut self) {
        let needs_sat = self.builder.use_var(self.sm_needs_sat_var);
        let sr = self.load_reg(reg::SR);
        let sr = self.or_vl(sr, needs_sat);
        self.store_reg(reg::SR, sr);
    }

    /// Compute V from MAC accumulation overflow (C unchanged).
    /// Uses standard signed overflow formula: both input signs agree but differ from result.
    pub(super) fn mac_set_vl(&mut self, product: Value, acc: Value, result: Value) {
        let sign_s = self.extract_bit_i64(product, 55);
        let sign_d = self.extract_bit_i64(acc, 55);
        let sign_r = self.extract_bit_i64(result, 55);
        let sr_xr = self.builder.ins().bxor(sign_s, sign_r);
        let dr_xr = self.builder.ins().bxor(sign_d, sign_r);
        let overflow = self.builder.ins().band(sr_xr, dr_xr);
        self.set_vl_overflow(overflow);
    }

    /// Update SR flags for 24-bit shift/rotate: clear C|N|Z|V, then set C from
    /// `carry` (bit-0 value), optionally set N from `n_val` (bit-0 value), set Z
    /// from `result == 0`. V is always cleared.
    pub(super) fn update_shift24_flags(
        &mut self,
        carry: Value,
        n_val: Option<Value>,
        result: Value,
    ) {
        let sr_new = self
            .clear_sr_flags((1u32 << sr::C) | (1u32 << sr::N) | (1u32 << sr::Z) | (1u32 << sr::V));
        // C = carry (already in bit 0 position, same as sr::C)
        let sr_new = self.builder.ins().bor(sr_new, carry);
        // N (if provided)
        let sr_new = if let Some(n) = n_val {
            let n_bit = self.shift_to_bit(n, sr::N);
            self.builder.ins().bor(sr_new, n_bit)
        } else {
            sr_new
        };
        // Z = (result == 0)
        let zero = self.builder.ins().iconst(types::I32, 0);
        let is_zero = self.builder.ins().icmp(IntCC::Equal, result, zero);
        let z_val = self.builder.ins().uextend(types::I32, is_zero);
        let z_bit = self.shift_to_bit(z_val, sr::Z);
        let sr_new = self.builder.ins().bor(sr_new, z_bit);
        self.store_reg(reg::SR, sr_new);
    }

    /// Update V, C, L flags for a 56-bit add or subtract.
    ///
    /// `source` and `dest` are the original 56-bit values.
    /// `result` is the unmasked i64 result. `is_sub` selects the overflow formula.
    pub(super) fn update_vcl(&mut self, source: Value, dest: Value, result: Value, is_sub: bool) {
        // Carry = bit 56 of the unmasked result
        let carry = self.extract_bit_i64(result, 56);

        // Extract sign bits (bit 55)
        let sign_s = self.extract_bit_i64(source, 55);
        let sign_d = self.extract_bit_i64(dest, 55);
        let result56 = self.mask56(result);
        let sign_r = self.extract_bit_i64(result56, 55);

        // Overflow formula differs for add vs sub
        let overflow = if is_sub {
            // Sub: (sign_s ^ sign_d) & (sign_r ^ sign_d)
            let sd = self.builder.ins().bxor(sign_s, sign_d);
            let rd = self.builder.ins().bxor(sign_r, sign_d);
            self.builder.ins().band(sd, rd)
        } else {
            // Add: (sign_s ^ sign_r) & (sign_d ^ sign_r)
            let sr = self.builder.ins().bxor(sign_s, sign_r);
            let dr = self.builder.ins().bxor(sign_d, sign_r);
            self.builder.ins().band(sr, dr)
        };

        // Update SR: clear V|C, set C (bit 0), V and L (sticky)
        let sr_new = self.clear_sr_flags((1u32 << sr::V) | (1u32 << sr::C));
        let sr_new = self.builder.ins().bor(sr_new, carry); // C is bit 0
        let sr_new = self.or_vl(sr_new, overflow);
        self.store_reg(reg::SR, sr_new);
    }

    /// Set the carry flag in SR.
    pub(super) fn set_carry(&mut self, c_bit: Value) {
        let sr_c = self.clear_sr_flags(1u32 << sr::C);
        // c_bit should be 0 or 1; C is bit 0 so mask only
        let one = self.builder.ins().iconst(types::I32, 1);
        let c_masked = self.builder.ins().band(c_bit, one);
        let sr_new = self.builder.ins().bor(sr_c, c_masked); // C is bit 0
        self.store_reg(reg::SR, sr_new);
    }

    /// Clear the V (overflow) flag in SR.
    pub(super) fn clear_v_flag(&mut self) {
        let sr = self.clear_sr_flags(1u32 << sr::V);
        self.store_reg(reg::SR, sr);
    }

    pub(super) fn update_vcl_sub(&mut self, source: Value, dest: Value, result: Value) {
        self.update_vcl(source, dest, result, true);
    }

    pub(super) fn update_vcl_add(&mut self, source: Value, dest: Value, result: Value) {
        self.update_vcl(source, dest, result, false);
    }
}
