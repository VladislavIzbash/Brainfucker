use inkwell::basic_block::BasicBlock;
use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::module::{Linkage, Module};
use inkwell::targets::TargetData;
use inkwell::values::{FunctionValue, PointerValue};
use inkwell::{AddressSpace, IntPredicate};
use inkwell::passes::PassManager;

struct Codegen<'ctx> {
    ctx: &'ctx Context,
    builder: Builder<'ctx>,
    target_data: &'ctx TargetData,
    fns: ExternalFns<'ctx>,
    counter: PointerValue<'ctx>,
    mem_ptr: PointerValue<'ctx>,
    loop_stack: Vec<BasicBlock<'ctx>>,
}

struct ExternalFns<'ctx> {
    putchar: FunctionValue<'ctx>,
    getchar: FunctionValue<'ctx>,
    calloc: FunctionValue<'ctx>,
    free: FunctionValue<'ctx>,
}

impl<'ctx> ExternalFns<'ctx> {
    fn new(ctx: &'ctx Context, module: &Module<'ctx>, target_data: &TargetData) -> Self {
        let putchar = ctx.i32_type().fn_type(&[ctx.i32_type().into()], false);
        let putchar = module.add_function("putchar", putchar, Some(Linkage::External));

        let getchar = ctx.i32_type().fn_type(&[], false);
        let getchar = module.add_function("getchar", getchar, Some(Linkage::External));

        let size_t = ctx.ptr_sized_int_type(&target_data, None);
        let i8_ptr = ctx.i8_type().ptr_type(AddressSpace::Generic);

        let calloc = i8_ptr.fn_type(&[size_t.into(), size_t.into()], false);
        let calloc = module.add_function("calloc", calloc, Some(Linkage::External));

        let free = ctx.void_type().fn_type(&[i8_ptr.into()], false);
        let free = module.add_function("free", free, Some(Linkage::External));

        Self { putchar, getchar, calloc, free }
    }
}

impl<'ctx> Codegen<'ctx> {
    fn gen_startup(&mut self, heap_size: u64) {
        let i32_t = self.ctx.i32_type();
        self.counter = self.builder.build_alloca(i32_t, "counter_alloc");
        self.builder.build_store(self.counter, i32_t.const_zero());

        let size_t = self.ctx.ptr_sized_int_type(&self.target_data, None);

        let num = size_t.const_int(heap_size, false);
        let size = size_t.const_int(1, false);

        self.mem_ptr = self.builder
            .build_call(self.fns.calloc, &[num.into(), size.into()], "calloc")
            .try_as_basic_value()
            .left()
            .unwrap()
            .into_pointer_value();
    }

    fn get_cell_ptr(&self) -> PointerValue<'ctx> {
        let counter = self.builder.build_load(self.counter, "counter_load")
            .into_int_value();

        unsafe {
            self.builder.build_gep(self.mem_ptr, &[counter], "get_cell_ptr")
        }
    }

    fn gen_move_right(&mut self) {
        let counter = self.builder.build_load(self.counter, "mr_counter_load")
            .into_int_value();
        let one = self.ctx.i32_type().const_int(1, false);

        let counter = self.builder.build_int_add(counter, one, "inc_counter");

        self.builder.build_store(self.counter, counter);
    }

    fn gen_move_left(&mut self) {
        let counter = self.builder.build_load(self.counter, "ml_counter_load")
            .into_int_value();
        let one = self.ctx.i32_type().const_int(1, false);

        let counter = self.builder.build_int_sub(counter, one, "dec_counter");

        self.builder.build_store(self.counter, counter);
    }

    fn gen_output(&self) {
        let cell_val = self.builder.build_load(self.get_cell_ptr(), "outp_load_cell")
            .into_int_value();
        let cell_val = self.builder.build_int_z_extend(cell_val, self.ctx.i32_type(), "outp_zero_ext");

        self.builder.build_call(self.fns.putchar, &[cell_val.into()], "putchar");
    }

    fn gen_input(&self) {
        let cell_ptr = self.get_cell_ptr();

        let inp = self.builder
            .build_call(self.fns.getchar, &[], "getchar")
            .try_as_basic_value()
            .left()
            .unwrap()
            .into_int_value();
        let inp = self.builder.build_int_truncate(inp, self.ctx.i8_type(), "inp_trunc");

        self.builder.build_store(cell_ptr, inp);
    }

    fn gen_increment_cell(&self) {
        let val_ptr = self.get_cell_ptr();
        let val = self.builder
            .build_load(val_ptr, "inc_load_cell")
            .into_int_value();

        let val = self.builder
            .build_int_add(val, self.ctx.i8_type().const_int(1, false), "inc_value");

        self.builder.build_store(val_ptr, val);
    }

    fn gen_decrement_cell(&self) {
        let val_ptr = self.get_cell_ptr();
        let val = self.builder
            .build_load(val_ptr, "inc_load_cell")
            .into_int_value();

        let val = self.builder
            .build_int_sub(val, self.ctx.i8_type().const_int(1, false), "dec_value");

        self.builder.build_store(val_ptr, val);
    }

    fn gen_loop_entry(&mut self, func: FunctionValue) {
        let loop_block = self.ctx.append_basic_block(func, "loop");
        self.builder.build_unconditional_branch(loop_block);
        self.builder.position_at_end(loop_block);

        self.loop_stack.push(loop_block);
    }

    fn gen_loop_end(&mut self, func: FunctionValue) {
        let val = self.builder.build_load(self.get_cell_ptr(), "loop_load_cell")
            .into_int_value();
        let zero = self.ctx.i8_type().const_zero();
        let val_not_zero = self.builder
            .build_int_compare(IntPredicate::NE, val, zero, "zero_cmp");

        let after_block = self.ctx.append_basic_block(func, "loop_after");
        let loop_block = self.loop_stack.pop().unwrap();

        self.builder.build_conditional_branch(val_not_zero, loop_block, after_block);
        self.builder.position_at_end(after_block);
    }

    fn gen_exit(&self) {
        self.builder.build_call(self.fns.free, &[self.mem_ptr.into()], "free");

        let zero = self.ctx.i32_type().const_zero();
        self.builder.build_return(Some(&zero));
    }

    pub fn generate(&mut self, heap_size: u64, code: &str) {
        self.gen_startup(heap_size);

        let func = self.builder
            .get_insert_block()
            .unwrap()
            .get_parent()
            .unwrap();

        for ch in code.chars() {
            match ch {
                '>' => self.gen_move_right(),
                '<' => self.gen_move_left(),
                '+' => self.gen_increment_cell(),
                '-' => self.gen_decrement_cell(),
                '.' => self.gen_output(),
                ',' => self.gen_input(),
                '[' => self.gen_loop_entry(func),
                ']' => self.gen_loop_end(func),
                _ => {}
            }
        }

        self.gen_exit();
    }
}

fn run_passes(module: &Module) {
    let passes = PassManager::create(());

    passes.add_promote_memory_to_register_pass();
    passes.add_constant_merge_pass();
    passes.add_dead_arg_elimination_pass();
    passes.add_global_optimizer_pass();
    passes.add_strip_symbol_pass();
    passes.add_loop_vectorize_pass();
    passes.add_aggressive_dce_pass();
    passes.add_dead_store_elimination_pass();
    passes.add_scalarizer_pass();
    passes.add_merged_load_store_motion_pass();
    passes.add_new_gvn_pass();
    passes.add_ind_var_simplify_pass();
    passes.add_instruction_combining_pass();
    passes.add_cfg_simplification_pass();
    passes.add_loop_deletion_pass();
    passes.add_loop_unroll_pass();
    passes.add_licm_pass();
    passes.add_reassociate_pass();
    passes.add_verifier_pass();

    passes.run_on(module);
}

pub fn compile_module<'a>(
    ctx: &'a Context,
    target_data: &'a TargetData,
    name: &str,
    heap_size: u64,
    code: &str,
) -> Module<'a> {
    let module = ctx.create_module(name);
    let builder = ctx.create_builder();

    let main_fn = ctx.i32_type().fn_type(&[], false);
    let main_fn = module.add_function("main", main_fn, None);
    let main_entry = ctx.append_basic_block(main_fn, "entry");
    builder.position_at_end(main_entry);

    let mut codegen = Codegen {
        ctx,
        builder,
        target_data,
        fns: ExternalFns::new(ctx, &module, target_data),
        mem_ptr: ctx.i8_type().ptr_type(AddressSpace::Generic).const_zero(),
        counter: ctx.i32_type().ptr_type(AddressSpace::Generic).const_zero(),
        loop_stack: Vec::new(),
    };

    codegen.generate(heap_size, code);

    run_passes(&module);

    module
}
