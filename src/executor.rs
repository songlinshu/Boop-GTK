extern crate rusty_v8;

use crate::script::Script;
use rusty_v8 as v8;
use std::{cell::RefCell, ptr, rc::Rc};

pub struct Executor {
    // v8
    is_v8_initalized: bool,
    isolate: *mut v8::OwnedIsolate,
    handle_scope: *mut v8::HandleScope<'static, ()>,
    context: *mut v8::Local<'static, v8::Context>,
    scope: *mut v8::ContextScope<'static, v8::HandleScope<'static, v8::Context>>,

    // script
    script: Script,
    main_function: *mut v8::Local<'static, v8::Function>,
}

#[derive(Clone)]
pub struct ExecutionStatus {
    pub info: Option<String>,
    pub error: Option<String>,
}

pub enum TextReplacement {
    Full(String),
    Selection(String),
}

impl Executor {
    pub fn new(script: Script) -> Self {
        Executor {
            is_v8_initalized: false,
            isolate: ptr::null_mut(),
            handle_scope: ptr::null_mut(),
            context: ptr::null_mut(),
            scope: ptr::null_mut(),
            script,
            main_function: ptr::null_mut(),
        }
    }

    pub fn script(&self) -> &Script {
        &self.script
    }

    fn isolate(&self) -> &mut v8::OwnedIsolate {
        assert!(!self.isolate.is_null());
        unsafe { &mut *self.isolate }
    }

    fn scope(&self) -> &mut v8::ContextScope<'static, v8::HandleScope<'static, v8::Context>> {
        assert!(!self.scope.is_null());
        unsafe { &mut *self.scope }
    }

    fn main_function(&self) -> &mut v8::Local<'static, v8::Function> {
        assert!(!self.main_function.is_null());
        unsafe { &mut *self.main_function }
    }

    fn initalized_v8(&mut self) {
        assert!(!self.is_v8_initalized);
        assert!(self.isolate.is_null());
        assert!(self.handle_scope.is_null());
        assert!(self.context.is_null());
        assert!(self.scope.is_null());

        info!("initalizing isolate for {}", self.script().metadata().name);

        // set up execution context
        self.isolate = Box::into_raw(Box::new(v8::Isolate::new(Default::default())));
        self.handle_scope = Box::into_raw(Box::new(v8::HandleScope::new(unsafe {
            &mut *self.isolate
        })));
        self.context = Box::into_raw(Box::new(v8::Context::new(unsafe {
            &mut *self.handle_scope
        })));
        self.scope = Box::into_raw(Box::new(v8::ContextScope::new(
            unsafe { &mut *self.handle_scope },
            unsafe { *self.context },
        )));

        let status_slot: Rc<RefCell<ExecutionStatus>> = Rc::new(RefCell::new(ExecutionStatus {
            info: None,
            error: None,
        }));
        self.isolate().set_slot(status_slot);
        // self.handle_scope().set_slot(status_slot);

        // complile and run script
        let code = v8::String::new(self.scope(), self.script.source()).unwrap();
        let compiled_script = v8::Script::compile(self.scope(), code, None).unwrap();
        compiled_script.run(self.scope()).unwrap();

        // extract main function
        let function_name = v8::String::new(self.scope(), "main").unwrap();
        self.main_function = unsafe {
            Box::into_raw(Box::new(v8::Local::cast(
                (*self.context)
                    .global(self.scope())
                    .get(self.scope(), function_name.into())
                    .unwrap(),
            )))
        };

        self.is_v8_initalized = true;
    }

    pub fn execute(
        &mut self,
        full_text: &str,
        selection: Option<&str>,
    ) -> (ExecutionStatus, TextReplacement) {
        if !self.is_v8_initalized {
            self.initalized_v8();
        }

        // create postInfo and postError functions
        let post_info = v8::Function::new(
            self.scope(),
            |scope: &mut v8::HandleScope,
             args: v8::FunctionCallbackArguments,
             mut rv: v8::ReturnValue| {
                let info = args
                    .get(0)
                    .to_string(scope)
                    .unwrap()
                    .to_rust_string_lossy(scope);

                scope
                    .get_slot_mut::<Rc<RefCell<ExecutionStatus>>>()
                    .unwrap()
                    .borrow_mut()
                    .info
                    .replace(info);

                let undefined = v8::undefined(scope).into();
                rv.set(undefined)
            },
        )
        .unwrap();

        let post_error = v8::Function::new(
            self.scope(),
            |scope: &mut v8::HandleScope,
             args: v8::FunctionCallbackArguments,
             mut rv: v8::ReturnValue| {
                let error = args
                    .get(0)
                    .to_string(scope)
                    .unwrap()
                    .to_rust_string_lossy(scope);

                scope
                    .get_slot_mut::<Rc<RefCell<ExecutionStatus>>>()
                    .unwrap()
                    .borrow_mut()
                    .error
                    .replace(error);

                let undefined = v8::undefined(scope).into();
                rv.set(undefined)
            },
        )
        .unwrap();

        // prepare payload
        let payload = v8::Object::new(self.scope());

        let key_full_text = v8::String::new(self.scope(), "fullText").unwrap();
        let key_text = v8::String::new(self.scope(), "text").unwrap();
        let key_selection = v8::String::new(self.scope(), "selection").unwrap();
        let key_post_info = v8::String::new(self.scope(), "postInfo").unwrap();
        let key_post_error = v8::String::new(self.scope(), "postError").unwrap();

        {
            // fullText
            let payload_full_text = v8::String::new(self.scope(), full_text).unwrap();
            payload.set(self.scope(), key_full_text.into(), payload_full_text.into());

            // text
            let payload_text =
                v8::String::new(self.scope(), selection.unwrap_or(full_text)).unwrap();
            payload.set(self.scope(), key_text.into(), payload_text.into());

            // selection
            let payload_selection = v8::String::new(self.scope(), selection.unwrap_or("")).unwrap();
            payload.set(self.scope(), key_selection.into(), payload_selection.into());

            // postInfo
            payload.set(self.scope(), key_post_info.into(), post_info.into());

            // postError
            payload.set(self.scope(), key_post_error.into(), post_error.into());
        }

        // call main
        self.main_function()
            .call(self.scope(), payload.into(), &[payload.into()]);

        // extract result
        // TODO(mrbenshef): it would be better to use accessors/interseptors, so we don't have to
        // compare potentially very large strings. however, I can't figure out how to do this
        // without static RwLock's
        // NOTE(mrbenshef): doesn't seem like there is a way to create a setter on an object with
        // rusty_v8, so this might have to do for now.
        let new_text_value = payload
            .get(self.scope(), key_text.into())
            .unwrap()
            .to_string(self.scope())
            .unwrap()
            .to_rust_string_lossy(self.scope());
        let new_full_text_value = payload
            .get(self.scope(), key_full_text.into())
            .unwrap()
            .to_string(self.scope())
            .unwrap()
            .to_rust_string_lossy(self.scope());
        let new_selection_value = payload
            .get(self.scope(), key_selection.into())
            .unwrap()
            .to_string(self.scope())
            .unwrap()
            .to_rust_string_lossy(self.scope());

        // not quite sure what the correct behaviour here should be
        // right now the order of presidence is:
        // 1. fullText
        // 2. selection
        // 3. text (with select)
        // 4. text (without selection)
        let replacement = if new_full_text_value != full_text {
            info!("found full_text replacement");
            TextReplacement::Full(new_full_text_value)
        } else if selection.is_some() && new_selection_value != selection.unwrap() {
            info!("found selection replacement");
            TextReplacement::Selection(new_selection_value)
        } else if selection.is_some() {
            info!("found text (with selection) replacement");
            TextReplacement::Selection(new_text_value)
        } else {
            info!("found text (without selection) replacement");
            TextReplacement::Full(new_text_value)
        };

        let status_slot = self
            .isolate()
            .get_slot_mut::<Rc<RefCell<ExecutionStatus>>>()
            .unwrap();

        let status = (*status_slot).borrow().clone();
        // let status = ExecutionStatus {
        //     info: None,
        //     error: None,
        // };

        (status, replacement)
    }
}

impl Drop for Executor {
    fn drop(&mut self) {
        if !self.is_v8_initalized {
            return;
        }

        unsafe {
            Box::from_raw(self.scope);
            Box::from_raw(self.context);
            Box::from_raw(self.handle_scope);
            Box::from_raw(self.isolate);
        }
    }
}
