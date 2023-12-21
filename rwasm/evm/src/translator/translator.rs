use crate::translator::{
    host::Host,
    instruction_result::InstructionResult,
    instructions::opcode,
    translator::contract::Contract,
};
pub use analysis::BytecodeLocked;
use fluentbase_runtime::Runtime;
use fluentbase_rwasm::rwasm::{
    BinaryFormat,
    Compiler,
    CompilerConfig,
    FuncOrExport,
    ImportLinker,
    InstructionSet,
    ReducedModule,
};
use hashbrown::HashMap;
use log::debug;
use std::marker::PhantomData;

pub mod analysis;
pub mod contract;

#[derive()]
pub struct Translator<'a> {
    pub contract: Box<Contract>,
    pub instruction_pointer: *const u8,
    pub instruction_result: InstructionResult,
    import_linker: &'a ImportLinker,
    opcode_to_subroutine_data: HashMap<u8, SubroutineData>,
    inject_fuel_consumption: bool,
    subroutines_instruction_set: InstructionSet,
    _lifetime: PhantomData<&'a ()>,
}

pub struct SubroutineData {
    pub rel_entry_offset: u32,
    pub instruction_set: InstructionSet,
    pub begin_offset: usize,
    pub end_offset: usize,
}

pub struct SubroutineMeta {
    pub begin_offset: usize,
    pub end_offset: usize,
}

impl<'a> Translator<'a> {
    pub fn new(
        import_linker: &'a ImportLinker,
        inject_fuel_consumption: bool,
        contract: Box<Contract>,
    ) -> Self {
        let mut s = Self {
            instruction_pointer: contract.bytecode.as_ptr(),
            contract,
            instruction_result: InstructionResult::Continue,
            import_linker,
            opcode_to_subroutine_data: Default::default(),
            inject_fuel_consumption,
            subroutines_instruction_set: Default::default(),
            _lifetime: Default::default(),
        };
        s.init_code_snippets();
        s
    }

    pub fn get_import_linker(&self) -> &'a ImportLinker {
        self.import_linker
    }

    #[inline]
    pub fn opcode_prev(&self) -> u8 {
        unsafe { *(self.instruction_pointer.sub(1)) }
    }

    #[inline]
    pub fn opcode_cur(&self) -> u8 {
        unsafe { *self.instruction_pointer }
    }

    #[inline]
    pub fn contract(&self) -> &Contract {
        &self.contract
    }

    #[inline]
    pub fn program_counter(&self) -> usize {
        // SAFETY: `instruction_pointer` should be at an offset from the start of the bytecode.
        // In practice this is always true unless a caller modifies the `instruction_pointer` field
        // manually.
        unsafe {
            self.instruction_pointer
                .offset_from(self.contract.bytecode.as_ptr()) as usize
        }
    }

    #[inline(always)]
    pub fn step<FN, H: Host>(&mut self, instruction_table: &[FN; 256], host: &mut H)
    where
        FN: Fn(&mut Translator<'_>, &mut H),
    {
        // Get current opcode.
        let opcode = unsafe { *self.instruction_pointer };

        self.instruction_pointer_inc(1);

        // execute instruction.
        (instruction_table[opcode as usize])(self, host)
    }

    pub fn instruction_pointer_inc(&mut self, offset: usize) {
        // Safety: In analysis we are doing padding of bytecode so that we are sure that last
        // byte instruction is STOP so we are safe to just increment program_counter bcs on last
        // instruction it will do noop and just stop execution of this contract
        self.instruction_pointer = unsafe { self.instruction_pointer.offset(offset as isize) };
    }

    pub fn run<FN, H: Host>(
        &mut self,
        instruction_table: &[FN; 256],
        host: &mut H,
    ) -> InstructionResult
    where
        FN: Fn(&mut Translator<'_>, &mut H),
    {
        while self.instruction_result == InstructionResult::Continue {
            self.step(instruction_table, host);
        }
        self.instruction_result
    }

    fn init_code_snippets(&mut self) {
        let mut initiate_subroutines = |opcode: u8, wasm_binary: &[u8], fn_name: &'static str| {
            if self.opcode_to_subroutine_data.contains_key(&opcode) {
                panic!(
                    "code snippet for opcode 0x{:x?} already exists (decimal: {})",
                    opcode, opcode
                );
            }
            let import_linker = Runtime::<()>::new_linker();
            let mut compiler = Compiler::new_with_linker(
                wasm_binary,
                CompilerConfig::default()
                    .fuel_consume(self.inject_fuel_consumption)
                    .translate_sections(false)
                    .type_check(false),
                Some(&import_linker),
            )
            .unwrap();
            let fn_idx = compiler
                .resolve_func_index(&FuncOrExport::Export(fn_name))
                .unwrap()
                .unwrap();
            compiler.translate(FuncOrExport::Func(fn_idx)).unwrap();
            let fn_beginning_offset = *compiler.resolve_func_beginning(fn_idx).unwrap();
            // let fn_beginning_offset = 0;
            let rwasm_binary = compiler.finalize().unwrap();
            let instruction_set = ReducedModule::new(&rwasm_binary)
                .unwrap()
                .bytecode()
                .clone();
            debug!(
                "\nsubroutine_instruction_set (fn_name '{}' opcode 0x{:x?} len {} fn_idx {} fn_beginning_offset {}): \n{}\n",
                fn_name,
                opcode,
                instruction_set.instr.len(),
                fn_idx,
                fn_beginning_offset,
                instruction_set.trace(),
            );
            let l = self.subroutines_instruction_set.instr.len();
            let subroutine_data = SubroutineData {
                rel_entry_offset: fn_beginning_offset,
                begin_offset: l,
                end_offset: l + instruction_set.len() as usize - 1,
                instruction_set,
            };
            self.subroutines_instruction_set
                .extend(&subroutine_data.instruction_set);
            self.opcode_to_subroutine_data
                .insert(opcode, subroutine_data);
        };

        [
            (
                opcode::EXP,
                "../rwasm-code-snippets/bin/arithmetic_exp.wat",
                "arithmetic_exp",
            ),
            (
                opcode::MOD,
                "../rwasm-code-snippets/bin/arithmetic_mod.wat",
                "arithmetic_mod",
            ),
            (
                opcode::SMOD,
                "../rwasm-code-snippets/bin/arithmetic_smod.wat",
                "arithmetic_smod",
            ),
            (
                opcode::MUL,
                "../rwasm-code-snippets/bin/arithmetic_mul.wat",
                "arithmetic_mul",
            ),
            (
                opcode::MULMOD,
                "../rwasm-code-snippets/bin/arithmetic_mulmod.wat",
                "arithmetic_mulmod",
            ),
            (
                opcode::ADD,
                "../rwasm-code-snippets/bin/arithmetic_add.wat",
                "arithmetic_add",
            ),
            (
                opcode::ADDMOD,
                "../rwasm-code-snippets/bin/arithmetic_addmod.wat",
                "arithmetic_addmod",
            ),
            (
                opcode::SIGNEXTEND,
                "../rwasm-code-snippets/bin/arithmetic_signextend.wat",
                "arithmetic_signextend",
            ),
            (
                opcode::SUB,
                "../rwasm-code-snippets/bin/arithmetic_sub.wat",
                "arithmetic_sub",
            ),
            (
                opcode::DIV,
                "../rwasm-code-snippets/bin/arithmetic_div.wat",
                "arithmetic_div",
            ),
            (
                opcode::SDIV,
                "../rwasm-code-snippets/bin/arithmetic_sdiv.wat",
                "arithmetic_sdiv",
            ),
            (
                opcode::SHL,
                "../rwasm-code-snippets/bin/bitwise_shl.wat",
                "bitwise_shl",
            ),
            (
                opcode::SHR,
                "../rwasm-code-snippets/bin/bitwise_shr.wat",
                "bitwise_shr",
            ),
            (
                opcode::NOT,
                "../rwasm-code-snippets/bin/bitwise_not.wat",
                "bitwise_not",
            ),
            (
                opcode::AND,
                "../rwasm-code-snippets/bin/bitwise_and.wat",
                "bitwise_and",
            ),
            (
                opcode::OR,
                "../rwasm-code-snippets/bin/bitwise_or.wat",
                "bitwise_or",
            ),
            (
                opcode::XOR,
                "../rwasm-code-snippets/bin/bitwise_xor.wat",
                "bitwise_xor",
            ),
            (
                opcode::EQ,
                "../rwasm-code-snippets/bin/bitwise_eq.wat",
                "bitwise_eq",
            ),
            (
                opcode::LT,
                "../rwasm-code-snippets/bin/bitwise_lt.wat",
                "bitwise_lt",
            ),
            (
                opcode::SLT,
                "../rwasm-code-snippets/bin/bitwise_slt.wat",
                "bitwise_slt",
            ),
            (
                opcode::GT,
                "../rwasm-code-snippets/bin/bitwise_gt.wat",
                "bitwise_gt",
            ),
            (
                opcode::SGT,
                "../rwasm-code-snippets/bin/bitwise_sgt.wat",
                "bitwise_sgt",
            ),
            (
                opcode::SAR,
                "../rwasm-code-snippets/bin/bitwise_sar.wat",
                "bitwise_sar",
            ),
            (
                opcode::BYTE,
                "../rwasm-code-snippets/bin/bitwise_byte.wat",
                "bitwise_byte",
            ),
            (
                opcode::ISZERO,
                "../rwasm-code-snippets/bin/bitwise_iszero.wat",
                "bitwise_iszero",
            ),
            (
                opcode::MSTORE,
                "../rwasm-code-snippets/bin/memory_mstore.wat",
                "memory_mstore",
            ),
            (
                opcode::MSTORE8,
                "../rwasm-code-snippets/bin/memory_mstore8.wat",
                "memory_mstore8",
            ),
            (
                opcode::POP,
                "../rwasm-code-snippets/bin/stack_pop.wat",
                "stack_pop",
            ),
            // (
            //     opcode::ADDRESS,
            //     "../rwasm-code-snippets/bin/system_address.wat",
            //     "system_address",
            // ),
            // (
            //     opcode::CALLER,
            //     "../rwasm-code-snippets/bin/system_caller.wat",
            //     "system_caller",
            // ),
            // (
            //     opcode::CALLVALUE,
            //     "../rwasm-code-snippets/bin/system_callvalue.wat",
            //     "system_callvalue",
            // ),
            (
                opcode::KECCAK256,
                "../rwasm-code-snippets/bin/system_keccak.wat",
                "system_keccak",
            ),
        ]
        .map(|v| {
            let opcode = v.0;
            let file_path = v.1;
            let fn_name = v.2;
            let bytecode = wat::parse_file(file_path).unwrap();
            initiate_subroutines(opcode, &bytecode, fn_name);
        });
    }

    pub fn opcode_to_subroutine_data(&self) -> &HashMap<u8, SubroutineData> {
        &self.opcode_to_subroutine_data
    }

    pub fn subroutine_data(&self, opcode: u8) -> Option<&SubroutineData> {
        self.opcode_to_subroutine_data.get(&opcode)
    }

    pub fn subroutines_instruction_set(&self) -> &InstructionSet {
        &self.subroutines_instruction_set
    }
}
