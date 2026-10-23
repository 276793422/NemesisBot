//! disasm —— iced-x86 反汇编封装（T3.1）。
//!
//! 只引 decoder 面：块解码 + 指令属性抽取（控制流类别/立即数/直接分支
//! 目标/rip 相对目标/nop 识别）。x64+x86 双模式按 machine 选 bitness。
//! 坏字节停块（保守解码：宁可少解不错解）。
//! 控制流分类走 `flow_control()`（iced-x86 1.21 无 is_near_branch——
//! 直接分支目标经 near_branch_target()，非直接分支返回 0 → None）。

use iced_x86::{Decoder, DecoderOptions, FlowControl, Instruction, Mnemonic, OpKind};

/// 目标架构（bitness 是 iced-x86 的 Decoder 入参）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    X64,
    X86,
}

impl Arch {
    pub fn bitness(self) -> u32 {
        match self {
            Arch::X64 => 64,
            Arch::X86 => 32,
        }
    }
    pub fn from_machine(machine: u16) -> Option<Arch> {
        match machine {
            0x8664 => Some(Arch::X64),
            0x14C => Some(Arch::X86),
            _ => None,
        }
    }
}

/// 解码后的单条指令属性（引擎内部流转形态；不含完整编码文本——
/// package 层按需生成有界摘要，不给裸全量汇编）。
#[derive(Debug, Clone)]
pub struct Insn {
    /// 所在 RVA。
    pub rva: u32,
    /// 指令长度（字节）。
    pub len: u32,
    /// 控制流类别（iced-x86 FlowControl）。
    pub flow: FlowControl,
    pub mnemonic: Mnemonic,
    /// 直接立即数（mov reg, imm32 等；地址性与数据性不区分，归因在 xref）。
    pub imm: Option<u64>,
    /// 近分支直接目标（直接 call/jcc 的 rel32/rel8 解析；非直接分支 None）。
    pub branch_target: Option<u32>,
    /// rip 相对内存操作数的绝对目标（RVA）。
    pub rip_rel_target: Option<u32>,
    /// 操作数里的内存访问（存在 data ref 可能性）。
    pub has_mem_operand: bool,
}

impl Insn {
    pub fn is_call(&self) -> bool {
        matches!(self.flow, FlowControl::Call | FlowControl::IndirectCall)
    }
    pub fn is_jmp(&self) -> bool {
        matches!(
            self.flow,
            FlowControl::UnconditionalBranch | FlowControl::IndirectBranch
        )
    }
    /// 条件跳转（jcc + loop 族 + jcxz 族——iced-x86 同类合并）。
    pub fn is_jcc(&self) -> bool {
        self.flow == FlowControl::ConditionalBranch
    }
    pub fn is_ret(&self) -> bool {
        self.flow == FlowControl::Return
    }
    pub fn is_nop(&self) -> bool {
        self.mnemonic == Mnemonic::Nop
    }
    /// 间接分支（jmp/call 寄存器或内存——递归下降不追，保守停）。
    pub fn is_indirect_branch(&self) -> bool {
        (self.is_call() || self.is_jmp()) && self.branch_target.is_none()
    }
}

/// 块解码：从 `bytes`（RVA 起）线性解码至多 `max_insns` 条或坏字节。
/// 返回已解码指令（坏字节之前的全部）。
pub fn decode_block(arch: Arch, bytes: &[u8], rva: u32, max_insns: usize) -> Vec<Insn> {
    let mut out = Vec::new();
    let mut dec = Decoder::with_ip(arch.bitness(), bytes, rva as u64, DecoderOptions::NONE);
    let mut insn = Instruction::default();
    while out.len() < max_insns && dec.can_decode() {
        dec.decode_out(&mut insn);
        if insn.is_invalid() {
            break; // 坏字节：保守停块
        }
        let flow = insn.flow_control();
        // 直接近分支：Call / 无条件 / 条件三类；间接/返回不带目标
        let branch_target = match flow {
            FlowControl::Call
            | FlowControl::UnconditionalBranch
            | FlowControl::ConditionalBranch => {
                let t = insn.near_branch_target();
                if t != 0 { Some(t as u32) } else { None }
            }
            _ => None,
        };
        let mut cur = Insn {
            rva: insn.ip32(),
            len: insn.len() as u32,
            flow,
            mnemonic: insn.mnemonic(),
            imm: None,
            branch_target,
            rip_rel_target: None,
            has_mem_operand: false,
        };
        // 立即数 + 内存操作数扫描
        for i in 0..insn.op_count() {
            match insn.op_kind(i) {
                OpKind::Immediate8
                | OpKind::Immediate8to16
                | OpKind::Immediate8to32
                | OpKind::Immediate8to64
                | OpKind::Immediate16
                | OpKind::Immediate32
                | OpKind::Immediate32to64
                | OpKind::Immediate64
                    if cur.imm.is_none() =>
                {
                    cur.imm = Some(insn.immediate(i));
                }
                OpKind::Memory => {
                    cur.has_mem_operand = true;
                    if insn.memory_base() == iced_x86::Register::RIP {
                        cur.rip_rel_target = Some(insn.ip_rel_memory_address() as u32);
                    }
                }
                _ => {}
            }
        }
        out.push(cur);
    }
    out
}

/// 前导 nop 串长度（字节数）——nopsled 候选启发式素材。
/// 只认纯 nop 指令串（0x90 / 多字节 nop），长度按指令累加。
pub fn leading_nop_bytes(arch: Arch, bytes: &[u8], rva: u32, max: usize) -> usize {
    let mut off = 0usize;
    let mut total = 0usize;
    while total < max {
        let end = bytes.len().min(off + 16);
        if off >= end {
            break;
        }
        let insns = decode_block(arch, &bytes[off..end], rva + off as u32, 1);
        match insns.first() {
            Some(i) if i.is_nop() => {
                total += i.len as usize;
                off += i.len as usize;
            }
            _ => break,
        }
    }
    total
}
