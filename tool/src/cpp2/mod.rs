mod formatter;
mod header;
mod ty;

use crate::{c2::CContext, common::{ErrorStore, FileMap}, ApiInfo};
use diplomat_core::hir::TypeContext;
use formatter::Cpp2Formatter;

/// This is the main object that drives this backend. Most execution steps
/// for this backend will be found as methods on this context
pub struct Cpp2Context<'tcx> {
    pub tcx: &'tcx TypeContext,
    pub c: CContext<'tcx>,
    pub formatter: Cpp2Formatter<'tcx>,
    pub errors: ErrorStore<'tcx, String>,
}

impl<'tcx> Cpp2Context<'tcx> {
    pub fn new(tcx: &'tcx TypeContext, files: FileMap) -> Self {
        Cpp2Context {
            tcx,
            c: CContext::new(tcx, files),
            formatter: Cpp2Formatter::new(tcx),
            errors: ErrorStore::default(),
        }
    }

    /// Run file generation
    ///
    /// Will populate self.files as a result
    pub fn run(&self, api_info: Option<&ApiInfo>) {
        self.c.files.add_file(
            "diplomat_runtime.hpp".into(),
            crate::cpp::RUNTIME_HPP.into(),
        );
        for (id, ty) in self.tcx.all_types() {
            self.gen_ty(id, ty, api_info)
        }
    }

    // further methods can be found in ty.rs and formatter.rs
}
