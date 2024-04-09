use super::header::Header;
use super::CContext;
use diplomat_core::hir::{self, OpaqueOwner, StructPathLike, TyPosition, Type, TypeDef, TypeId};
use std::borrow::Cow;
use std::cell::RefCell;
use std::fmt::Write;

impl<'tcx> super::CContext<'tcx> {
    pub fn gen_ty(&self, id: TypeId, ty: TypeDef<'tcx>) {
        if ty.attrs().disable {
            // Skip type if disabled
            return;
        }
        let decl_header_path = self.formatter.fmt_decl_header_path(id);
        let decl_header = Header::new(decl_header_path.clone());
        let impl_header_path = self.formatter.fmt_impl_header_path(id);
        let impl_header = Header::new(impl_header_path.clone());

        let mut context = TyGenContext {
            cx: self,
            decl_header: RefCell::new(decl_header),
            impl_header: RefCell::new(impl_header),
        };

        let _guard = self.errors.set_context_ty(ty.name().as_str().into());
        match ty {
            TypeDef::Enum(e) => context.gen_enum_def(e, id),
            TypeDef::Opaque(o) => context.gen_opaque_def(o, id),
            TypeDef::Struct(s) => context.gen_struct_def(s, id),
            TypeDef::OutStruct(s) => context.gen_struct_def(s, id),
            _ => unreachable!("unknown AST/HIR variant"),
        }

        let mut method_generated = false;

        for method in ty.methods() {
            if method.attrs.disable {
                // Skip method if disabled
                continue;
            }
            let _guard = self.errors.set_context_method(
                self.formatter.fmt_type_name_diagnostics(id),
                method.name.as_str().into(),
            );
            context.gen_method(id, method, false);

            method_generated = true;
        }

        if let TypeDef::Opaque(_) = ty {
            context.gen_dtor(id, false);

            method_generated = true;
        }

        // Only generate the api if a method was generated
        if method_generated {
            context.gen_api(id, ty);
        }

        // In some cases like generating decls for `self` parameters,
        // a header will get its own includes. Instead of
        // trying to avoid pushing them, it's cleaner to just pull them out
        // once done
        context.decl_header.borrow_mut().includes.remove(&*decl_header_path);
        context.impl_header.borrow_mut().includes.remove(&*impl_header_path);
        context.impl_header.borrow_mut().includes.remove(&*decl_header_path);

        context.impl_header.borrow_mut().decl_include = Some(decl_header_path.clone());

        self.files
            .add_file(decl_header_path, context.decl_header.borrow().to_string());
        self.files
            .add_file(impl_header_path, context.impl_header.borrow().to_string());
    }

    pub fn gen_result(&self, name: &str, ty: ResultType) {
        let _guard = self
            .errors
            .set_context_ty(self.formatter.fmt_result_for_diagnostics(ty).into());
        let header_path = self.formatter.fmt_result_header_path(name);
        let header = Header::new(header_path.clone());
        let dummy_header = Header::new("".to_string());
        let mut context = TyGenContext {
            cx: self,
            // NOTE: Only one header for results
            decl_header: RefCell::new(header),
            impl_header: RefCell::new(dummy_header),
        };
        context.gen_result(name, ty);
        self.files.add_file(header_path, context.decl_header.borrow().to_string());
    }

    pub fn gen_ty_decl<'a, P: TyPosition>(&self, id: TypeId, ty: &Type<P>, ident: Cow<'a, str>) -> Vec<(Cow<'_, str>, Cow<'a, str>)> {
        let decl_header_path = self.formatter.fmt_decl_header_path(id);
        let header = Header::new(decl_header_path);
        let dummy_header = Header::new("".to_string());
        let context = TyGenContext {
            cx: self,
            // NOTE: Only one header for results
            decl_header: RefCell::new(header),
            impl_header: RefCell::new(dummy_header),
        };
        context.gen_ty_decl(ty, &ident, false)
    }
}
/// Simple wrapper type representing the return type of a fallible function
pub type ResultType<'tcx> = (Option<&'tcx hir::OutType>, Option<&'tcx hir::OutType>);

/// Context for generating a particular type's header
pub struct TyGenContext<'ccx, 'tcx> {
    pub cx: &'ccx CContext<'tcx>,
    pub decl_header: RefCell<Header>,
    pub impl_header: RefCell<Header>,
}

impl<'ccx, 'tcx: 'ccx> TyGenContext<'ccx, 'tcx> {
    pub fn gen_enum_def(&mut self, def: &'tcx hir::EnumDef, id: TypeId) {
        let ty_name = self.cx.formatter.fmt_type_name(id);
        writeln!(self.decl_header.borrow_mut(), "typedef enum {ty_name} {{").unwrap();
        for variant in def.variants.iter() {
            let enum_variant = self.cx.formatter.fmt_enum_variant(&ty_name, variant);
            let discriminant = variant.discriminant;
            writeln!(self.decl_header.borrow_mut(), "\t{enum_variant} = {discriminant},").unwrap();
        }
        write!(self.decl_header.borrow_mut(), "}} {ty_name};\n\n").unwrap();
    }

    pub fn gen_opaque_def(&mut self, _def: &'tcx hir::OpaqueDef, id: TypeId) {
        let ty_name = self.cx.formatter.fmt_type_name(id);
        write!(self.decl_header.borrow_mut(), "typedef struct {ty_name} {ty_name};\n\n").unwrap();
    }

    pub fn gen_struct_def<P: TyPosition>(&mut self, def: &'tcx hir::StructDef<P>, id: TypeId) {
        let ty_name = self.cx.formatter.fmt_type_name(id);
        writeln!(self.decl_header.borrow_mut(), "typedef struct {ty_name} {{").unwrap();
        for field in def.fields.iter() {
            let decls = self.gen_ty_decl(&field.ty, field.name.as_str(), true);
            for (decl_ty, decl_name) in decls {
                writeln!(self.decl_header.borrow_mut(), "\t{decl_ty} {decl_name};").unwrap();
            }
        }
        // reborrow to avoid borrowing across mutation
        write!(self.decl_header.borrow_mut(), "}} {ty_name};\n\n").unwrap();
    }

    pub fn gen_api(&mut self, id: TypeId, ty: TypeDef<'tcx>) {
        write!(self.impl_header.borrow_mut(), "\nstruct __{}_API__\n{{\n", ty.name().as_str()).unwrap();
        for method in ty.methods() {
            if method.attrs.disable {
                // Skip method if disabled
                continue;
            }
            self.gen_method(id, method, true);
        }

        if let TypeDef::Opaque(_) = ty {
            self.gen_dtor(id, true);
        }

        write!(self.impl_header.borrow_mut(), "}};\n\n").unwrap();
    }

    pub fn gen_method(&mut self, id: TypeId, method: &'tcx hir::Method, is_api: bool) {
        use diplomat_core::hir::{ReturnType, SuccessType};
        let method_name = self.cx.formatter.fmt_method_name(id, method, !is_api);
        let mut param_decls = Vec::new();
        if let Some(ref self_ty) = method.param_self {
            let self_ty = self_ty.ty.clone().into();
            param_decls = self.gen_ty_decl(&self_ty, "self", false);
        }

        for param in &method.params {
            let decls = self.gen_ty_decl(&param.ty, param.name.as_str(), false);
            param_decls.extend(decls);
        }

        let return_ty: Cow<str> = match method.output {
            ReturnType::Infallible(SuccessType::Unit) => "void".into(),
            ReturnType::Infallible(SuccessType::Writeable) => {
                param_decls.push(("DiplomatWriteable*".into(), "writeable".into()));
                "void".into()
            }
            ReturnType::Infallible(SuccessType::OutType(ref o)) => self.gen_ty_name(o, false, None),
            ReturnType::Fallible(ref ok, _) | ReturnType::Nullable(ref ok) => {
                // Result<T, ()> and Option<T> are the same on the ABI
                let err = if let ReturnType::Fallible(_, Some(ref e)) = method.output {
                    Some(e)
                } else {
                    None
                };
                let (ok_type_name, ok_ty) = match ok {
                    SuccessType::Writeable => {
                        param_decls.push(("DiplomatWriteable*".into(), "writeable".into()));
                        ("void".into(), None)
                    }
                    SuccessType::Unit => ("void".into(), None),
                    SuccessType::OutType(o) => {
                        (self.cx.formatter.fmt_type_name_uniquely(o), Some(o))
                    }
                    _ => unreachable!("unknown AST/HIR variant"),
                };
                let err_type_name = match err {
                    Some(o) => self.cx.formatter.fmt_type_name_uniquely(o),
                    None => "void".into(),
                };
                // todo push to results set
                let result_name = self
                    .cx
                    .formatter
                    .fmt_result_name(&ok_type_name, &err_type_name);
                self.impl_header.borrow_mut()
                    .includes
                    .insert(self.cx.formatter.fmt_result_header_path(&result_name));
                self.cx
                    .result_store
                    .borrow_mut()
                    .insert(result_name.clone(), (ok_ty, err));
                result_name.into()
            }
            _ => unreachable!("unknown AST/HIR variant"),
        };

        let mut params = String::new();
        let mut first = true;
        for (decl_ty, decl_name) in param_decls {
            let comma = if first {
                first = false;
                ""
            } else {
                ", "
            };
            if decl_name.is_empty() {
                write!(&mut params, "{comma}{decl_ty}").unwrap();
            } else {
                write!(&mut params, "{comma}{decl_ty} {decl_name}").unwrap();
            }
        }

        if is_api {
            write!(self.impl_header.borrow_mut(), "  {return_ty} (*{method_name})({params});\n").unwrap();
        } else {
            write!(self.impl_header.borrow_mut(), "{return_ty} {method_name}({params});\n\n").unwrap();
        }
    }

    pub fn gen_dtor(&mut self, id: TypeId, is_api: bool) {
        let ty_name = self.cx.formatter.fmt_type_name(id);
        let dtor_name = self.cx.formatter.fmt_dtor_name(id);
        if is_api {
            write!(self.impl_header.borrow_mut(), "  void (*{dtor_name})({ty_name}* self);\n").unwrap();
        } else {
            write!(self.impl_header.borrow_mut(), "void {dtor_name}({ty_name}* self);\n\n").unwrap();
        }
    }

    pub fn gen_result(&mut self, name: &str, ty: ResultType) {
        let ok_line = if let Some(ok) = ty.0 {
            let ok_name = self.gen_ty_name(ok, true, None);
            format!("\t\t{ok_name} ok;\n")
        } else {
            "".into()
        };
        let err_line = if let Some(err) = ty.1 {
            let err_name = self.gen_ty_name(err, true, None);
            format!("\t\t{err_name} err;\n")
        } else {
            "".into()
        };

        let union_def = if ty.0.is_some() || ty.1.is_some() {
            format!("\tunion {{\n{ok_line}{err_line}\t}};\n")
        } else {
            "".into()
        };

        writeln!(
            self.decl_header.borrow_mut(),
            "typedef struct {name} {{\n{union_def}\tbool is_ok;\n}} {name};"
        )
        .unwrap();
    }

    /// Generates a list of decls for a given type, returned as (type, name)
    ///
    /// Might return multiple in the case of slices and strings. The `is_struct` parameter
    /// affects whether the decls are generated for a struct field or method
    pub fn gen_ty_decl<'a, P: TyPosition>(
        &self,
        ty: &Type<P>,
        ident: &str,
        is_struct: bool,
    ) -> Vec<(Cow<'ccx, str>, Cow<'a, str>)> {
        let param_name = self.cx.formatter.fmt_param_name(ident);
        match ty {
            Type::Slice(hir::Slice::Str(
                _,
                hir::StringEncoding::UnvalidatedUtf8 | hir::StringEncoding::Utf8,
            )) if !is_struct => {
                vec![
                    ("const char*".into(), format!("{param_name}_data").into()),
                    ("size_t".into(), format!("{param_name}_len").into()),
                ]
            }
            Type::Slice(hir::Slice::Str(_, hir::StringEncoding::UnvalidatedUtf16))
                if !is_struct =>
            {
                vec![
                    (
                        "const char16_t*".into(),
                        format!("{param_name}_data").into(),
                    ),
                    ("size_t".into(), format!("{param_name}_len").into()),
                ]
            }
            Type::Slice(hir::Slice::Primitive(b, p)) if !is_struct => {
                let prim = self.cx.formatter.fmt_primitive_as_c(*p);
                let ptr_type = self.cx.formatter.fmt_ptr(
                    &prim,
                    b.map(|b| b.mutability).unwrap_or(hir::Mutability::Mutable),
                );
                vec![
                    (
                        format!("{ptr_type}").into(),
                        format!("{param_name}_data").into(),
                    ),
                    ("size_t".into(), format!("{param_name}_len").into()),
                ]
            }
            Type::Slice(hir::Slice::Strs(encoding)) => {
                vec![
                    (
                        match encoding {
                            hir::StringEncoding::UnvalidatedUtf16 => "DiplomatStrs16View*",
                            _ => "DiplomatStrs8View*",
                        }
                        .into(),
                        format!("{param_name}_data").into(),
                    ),
                    ("size_t".into(), format!("{param_name}_len").into()),
                ]
            }
            Type::Func(..) => {
                let ty = self.gen_ty_name(ty, is_struct, Some(&param_name));
                vec![
                    (ty, "".into()),
                    ("void*".into(), "".into())
                ]
            }
            _ => {
                let ty = self.gen_ty_name(ty, is_struct, Some(&param_name));
                vec![(ty, format!("{param_name}").into())]
            }
        }
    }

    // Generate the C code for referencing a particular type.
    // Handles adding imports and such as necessary
    fn gen_ty_name<P: TyPosition>(&self, ty: &Type<P>, is_decl: bool, param_name: Option<&str>) -> Cow<'ccx, str> {
        let (id, ty_name) = match ty {
            Type::Primitive(prim) => (None, self.cx.formatter.fmt_primitive_as_c(*prim)),
            Type::Opaque(ref op) => {
                let op_id = op.tcx_id.into();
                let ty_name = self.cx.formatter.fmt_type_name(op_id);
                if self.cx.tcx.resolve_type(op_id).attrs().disable {
                    self.cx
                        .errors
                        .push_error(format!("Found usage of disabled type {ty_name}"))
                }
                // unwrap_or(mut) since owned pointers need to not be const
                let mutability = op.owner.mutability().unwrap_or(hir::Mutability::Mutable);
                let ret = self.cx.formatter.fmt_ptr(&ty_name, mutability);
                let header = if is_decl { &self.decl_header } else { &self.impl_header };
                header.borrow_mut()
                    .includes
                    .insert(self.cx.formatter.fmt_decl_header_path(op_id));
                (Some(op_id), ret.into_owned().into())
            }
            Type::Struct(ref st) => {
                let st_id = st.id();
                let ty_name = self.cx.formatter.fmt_type_name(st_id);
                if self.cx.tcx.resolve_type(st_id).attrs().disable {
                    self.cx
                        .errors
                        .push_error(format!("Found usage of disabled type {ty_name}"))
                }
                let ret = ty_name.clone();
                let header_path = self.cx.formatter.fmt_decl_header_path(st_id);
                let header = if is_decl { &self.decl_header } else { &self.impl_header };
                header.borrow_mut().includes.insert(header_path);
                (Some(st_id), ret)
            }
            Type::Enum(ref e) => {
                let id = e.tcx_id.into();
                let ty_name = self.cx.formatter.fmt_type_name(id);
                if self.cx.tcx.resolve_type(id).attrs().disable {
                    self.cx
                        .errors
                        .push_error(format!("Found usage of disabled type {ty_name}"))
                }
                let header_path = self.cx.formatter.fmt_decl_header_path(id);
                let header = if is_decl { &self.decl_header } else { &self.impl_header };
                header.borrow_mut().includes.insert(header_path);
                (Some(id), ty_name)
            }
            Type::Slice(ref s) => {
                let ptr_ty = match s {
                    hir::Slice::Str(
                        _,
                        hir::StringEncoding::UnvalidatedUtf8 | hir::StringEncoding::Utf8,
                    ) => "char".into(),
                    hir::Slice::Str(_, hir::StringEncoding::UnvalidatedUtf16) => "char16_t".into(),
                    hir::Slice::Primitive(_, prim) => self.cx.formatter.fmt_primitive_as_c(*prim),
                    hir::Slice::Strs(hir::StringEncoding::UnvalidatedUtf16) => {
                        "DiplomatStrs16View".into()
                    }
                    hir::Slice::Strs(_) => "DiplomatStrs8View".into(),
                    &_ => unreachable!("unknown AST/HIR variant"),
                };
                (
                    None,
                    format!("struct {{ const {ptr_ty}* data; size_t len; }}").into(),
                )
            }
            Type::Func(f) => {
                let outputs = f.output.as_ref()
                    .map(|o| self.gen_ty_decl(o, "", false)
                        .into_iter().map(|(_, ty)| ty).collect::<Vec<_>>()
                    )
                    .unwrap_or_default();
                let output = match (outputs.get(0), outputs.get(1)) {
                    (Some(o), None) => o,
                    (None, None) => &Cow::Borrowed("void"),
                    _ => unreachable!("unknown AST/HIR variant")
                };
                let inputs = std::iter::once((Cow::Borrowed("void*"), None))
                    .chain(f.inputs.iter()
                        .flat_map(|(ty, _)|
                            self.gen_ty_decl(ty, "", false).into_iter()
                                .map(|(ty, _)| (ty, None)/*(ty, if id.is_empty() { None } else { Some(id) })*/)
                        ))
                    .collect::<Vec<_>>();
                (None, self.cx.formatter.fmt_function_as_c(param_name, output, &inputs))
            },
            _ => unreachable!("unknown AST/HIR variant"),
        };
        // Todo(breaking): We can remove this requirement
        // and users will be forced to import more types
        if let Some(id) = id {
            if !is_decl {
                let header = if is_decl { &self.decl_header } else { &self.impl_header };
                header.borrow_mut()
                    .includes
                    .insert(self.cx.formatter.fmt_impl_header_path(id));
            }
        }
        ty_name
    }
}
