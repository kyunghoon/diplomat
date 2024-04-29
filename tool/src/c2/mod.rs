mod formatter;
mod header;
mod ty;

pub use self::formatter::CFormatter;

use crate::common::{ErrorStore, FileMap};
use crate::ApiInfo;
use diplomat_core::hir::TypeContext;
use std::cell::RefCell;
use std::collections::HashMap;

fn render_api(apiname: &str, entrypoint: &str, ty_names: &[&str]) -> String {
    let includes = ty_names.iter()
        .map(|n| format!("#include \"{}.h\"", n))
        .collect::<Vec<_>>()
        .join("\n");

    let members = ty_names.iter()
        .map(|n| format!("  __{0}_API__ {};", n))
        .collect::<Vec<_>>()
        .join("\n");
    format!(r##"#ifndef API_{apiname}_H
#define API_{apiname}_H

{includes}

#ifdef __cplusplus
namespace capi {{
extern "C" {{
#endif // __cplusplus

struct __Core_API__ {{
  void(*free)(void* ptr);
}};

struct {apiname}
{{
  __Core_API__ core;
{members}
}};

const {apiname}* {entrypoint}();

#ifdef __cplusplus
}} // extern "C"
}} // namespace capi
#endif // __cplusplus

#endif // API_{apiname}_H
"##)
}

/// This is the main object that drives this backend. Most execution steps
/// for this backend will be found as methods on this context
pub struct CContext<'tcx> {
    pub tcx: &'tcx TypeContext,
    pub formatter: CFormatter<'tcx>,
    pub files: FileMap,
    // The results needed by various methods
    pub result_store: RefCell<HashMap<String, ty::ResultType<'tcx>>>,

    pub errors: ErrorStore<'tcx, String>,
}

impl<'tcx> CContext<'tcx> {
    pub fn new(tcx: &'tcx TypeContext, files: FileMap) -> Self {
        CContext {
            tcx,
            files,
            formatter: CFormatter::new(tcx),
            result_store: Default::default(),
            errors: ErrorStore::default(),
        }
    }

    /// Run file generation
    ///
    /// Will populate self.files as a result
    pub fn run(&self, api_info: Option<&ApiInfo>) {
        self.files
            .add_file("diplomat_runtime.h".into(), crate::c::RUNTIME_H.into());
        for (id, ty) in self.tcx.all_types() {
            self.gen_ty(id, ty)
        }

        for (result_name, result_ty) in self.result_store.borrow().iter() {
            self.gen_result(result_name, *result_ty)
        }

        if let Some(ApiInfo { apiname, refresh_api_fn: entrypoint, .. }) = api_info { 
            let ty_names = self.tcx.all_types()
                .filter_map(|(_, ty)| if ty.attrs().disable || ty.methods().is_empty() { None } else { Some(ty.name().as_str()) })
                .collect::<Vec<_>>();

            self.files.add_file(
                "api.h".into(),
                render_api(apiname, entrypoint, &ty_names)
            );
        }
    }

    // further methods can be found in ty.rs and formatter.rs
}
