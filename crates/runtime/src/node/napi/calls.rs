use super::{Env, State, Status, chars, check};
use crate::jsc::Ptr;
use std::{ffi::c_char, ptr};

pub(super) type Callback = unsafe extern "C" fn(Env, *const Info) -> Ptr;

pub(super) struct Info {
    pub(super) this: Ptr,
    pub(super) target: Ptr,
    pub(super) args: *const [Ptr],
    pub(super) data: Ptr,
}

#[repr(C)]
pub(super) struct Property {
    name: *const c_char,
    key: Ptr,
    method: Option<Callback>,
    getter: Option<Callback>,
    setter: Option<Callback>,
    value: Ptr,
    attributes: u32,
    data: Ptr,
}

impl State {
    unsafe fn settle(&self, deferred: Ptr, value: Ptr, name: &str) -> Status {
        let env = self;
        let mut state = ptr::null_mut();
        check(
            unsafe { Self::napi_get_reference_value(env, deferred, &mut state) } == 0,
            1,
        )?;
        let callback = env.js("get", &[state, env.text(name)])?;
        check(
            unsafe { Self::napi_call_function(env, state, callback, 1, &value, ptr::null_mut()) }
                == 0,
            10,
        )?;
        check(
            unsafe { Self::napi_delete_reference(env, deferred) } == 0,
            1,
        )
    }

    api!(napi_get_property_names(env, object: Ptr, result: *mut Ptr) {
        env.object(object)?;
        env.value(result, env.js("keys", &[object])?)
    });
    api!(napi_get_prototype(env, object: Ptr, result: *mut Ptr) {
        env.object(object)?;
        env.value(result, env.js("prototype", &[object])?)
    });
    api!(napi_get_property(env, object: Ptr, key: Ptr, result: *mut Ptr) {
        env.object(object)?;
        env.value(result, env.js("get", &[object, key])?)
    });
    api!(napi_set_property(env, object: Ptr, key: Ptr, value: Ptr) {
        env.object(object)?;
        env.js("set", &[object, key, value])?;
        Ok(())
    });
    api!(napi_has_property(env, object: Ptr, key: Ptr, result: *mut bool) {
        env.object(object)?;
        check(!result.is_null(), 1)?;
        *result = env.truthy("has", &[object, key])?;
        Ok(())
    });
    api!(napi_delete_property(env, object: Ptr, key: Ptr, result: *mut bool) {
        env.object(object)?;
        let value = env.js("delete", &[object, key])?;
        if !result.is_null() {
            *result = (env.api.to_boolean)(env.ctx, value);
        }
        Ok(())
    });
    api!(napi_get_named_property(
        env,
        object: Ptr,
        name: *const c_char,
        result: *mut Ptr,
    ) {
        env.object(object)?;
        check(!name.is_null(), 1)?;
        env.value(result, env.access(object, Some(name), 0, None)?)
    });
    api!(napi_set_named_property(env, object: Ptr, name: *const c_char, value: Ptr) {
        env.object(object)?;
        check(!name.is_null(), 1)?;
        env.access(object, Some(name), 0, Some(value))?;
        Ok(())
    });
    api!(napi_has_named_property(
        env,
        object: Ptr,
        name: *const c_char,
        result: *mut bool,
    ) {
        env.object(object)?;
        check(!result.is_null(), 1)?;
        check(!name.is_null(), 1)?;
        let key = env.c_text(name);
        *result = env.truthy("has", &[object, key])?;
        Ok(())
    });
    api!(napi_get_element(env, object: Ptr, index: u32, result: *mut Ptr) {
        env.object(object)?;
        env.value(result, env.access(object, None, index, None)?)
    });
    api!(napi_set_element(env, object: Ptr, index: u32, value: Ptr) {
        env.object(object)?;
        env.access(object, None, index, Some(value))?;
        Ok(())
    });
    api!(napi_has_element(env, object: Ptr, index: u32, result: *mut bool) {
        env.object(object)?;
        check(!result.is_null(), 1)?;
        let key = env.number(index as f64);
        *result = env.truthy("has", &[object, key])?;
        Ok(())
    });
    api!(napi_delete_element(env, object: Ptr, index: u32, result: *mut bool) {
        env.object(object)?;
        let key = env.number(index as f64);
        let value = env.js("delete", &[object, key])?;
        if !result.is_null() {
            *result = (env.api.to_boolean)(env.ctx, value);
        }
        Ok(())
    });
    api!(napi_has_own_property(env, object: Ptr, key: Ptr, result: *mut bool) {
        env.object(object)?;
        check(!result.is_null(), 1)?;
        *result = env.truthy("own", &[object, key])?;
        Ok(())
    });
    api!(napi_get_array_length(env, value: Ptr, result: *mut u32) {
        check(!result.is_null(), 1)?;
        check(env.truthy("is_array", &[value])?, 8)?;
        let length = env.js("get", &[value, env.text("length")])?;
        *result = (env.api.to_number)(env.ctx, length, ptr::null_mut()) as u32;
        Ok(())
    });
    api!(napi_object_freeze(env, object: Ptr) {
        env.object(object)?;
        env.js("freeze", &[object])?;
        Ok(())
    });
    api!(napi_object_seal(env, object: Ptr) {
        env.object(object)?;
        env.js("seal", &[object])?;
        Ok(())
    });
    api!(napi_create_function(
        env,
        name: *const c_char,
        length: usize,
        callback: Option<Callback>,
        data: Ptr,
        result: *mut Ptr,
    ) {
        let callback = callback.ok_or(1)?;
        let name = String::from_utf8_lossy(chars(name, length));
        env.value(result, env.function(&name, callback, data)?)
    });
    api!(napi_get_cb_info(
        env,
        info: *const Info,
        argc: *mut usize,
        argv: *mut Ptr,
        this: *mut Ptr,
        data: *mut Ptr,
    ) {
        check(!info.is_null(), 1)?;
        let info = &*info;
        let args = &*info.args;
        if !argv.is_null() {
            check(!argc.is_null(), 1)?;
            for index in 0..*argc {
                *argv.add(index) = args.get(index).copied().unwrap_or_else(|| env.undefined());
            }
        }
        if !argc.is_null() {
            *argc = args.len();
        }
        if !this.is_null() {
            *this = info.this;
        }
        if !data.is_null() {
            *data = info.data;
        }
        Ok(())
    });
    api!(napi_get_new_target(env, info: *const Info, result: *mut Ptr) {
        check(!info.is_null() && !result.is_null(), 1)?;
        *result = (*info).target;
        Ok(())
    });
    api!(napi_call_function(
        env,
        this: Ptr,
        function: Ptr,
        argc: usize,
        argv: *const Ptr,
        result: *mut Ptr,
    ) {
        check(argc == 0 || !argv.is_null(), 1)?;
        check(env.kind(function)? == 7, 5)?;
        let mut args = Vec::with_capacity(argc + 2);
        args.extend_from_slice(&[function, this]);
        if argc > 0 {
            args.extend_from_slice(std::slice::from_raw_parts(argv, argc));
        }
        // JSC treats a primitive receiver as a missing one, so only object receivers skip the helper.
        check(args.iter().all(|arg| !arg.is_null()), 1)?;
        let value = if (env.api.value_type)(env.ctx, this) == 5 {
            let mut exception = ptr::null_mut();
            let value = (env.api.invoke)(env.ctx, function, this, argc, args.as_ptr().add(2), &mut exception);
            if !exception.is_null() {
                env.exception(exception);
                return Err(10);
            }
            env.keep(value)
        } else {
            env.js("call", &args)?
        };
        if !result.is_null() {
            env.value(result, value)?;
        }
        Ok(())
    });
    api!(napi_new_instance(
        env,
        constructor: Ptr,
        argc: usize,
        argv: *const Ptr,
        result: *mut Ptr,
    ) {
        check(argc == 0 || !argv.is_null(), 1)?;
        let mut args = vec![constructor];
        if argc > 0 {
            args.extend_from_slice(std::slice::from_raw_parts(argv, argc));
        }
        env.value(result, env.js("construct", &args)?)
    });
    api!(napi_run_script(env, script: Ptr, result: *mut Ptr) {
        check(env.kind(script)? == 4, 3)?;
        let string = (env.api.to_string)(env.ctx, script, ptr::null_mut());
        let mut exception = ptr::null_mut();
        let value = (env.api.evaluate)(
            env.ctx,
            string,
            ptr::null_mut(),
            ptr::null_mut(),
            1,
            &mut exception,
        );
        (env.api.string_release)(string);
        if !exception.is_null() {
            env.exception(exception);
            return Err(10);
        }
        env.value(result, value)
    });
    api!(napi_create_error(env, code: Ptr, message: Ptr, result: *mut Ptr) {
        check(env.kind(message)? == 4, 3)?;
        env.value(result, env.error("Error", code, message)?)
    });
    api!(napi_throw_error(env, code: *const c_char, message: *const c_char) {
        check(!message.is_null(), 1)?;
        let code = if code.is_null() { ptr::null_mut() } else { env.c_text(code) };
        env.exception(env.error("Error", code, env.c_text(message))?);
        Ok(())
    });
    api!(napi_create_type_error(env, code: Ptr, message: Ptr, result: *mut Ptr) {
        check(env.kind(message)? == 4, 3)?;
        env.value(result, env.error("TypeError", code, message)?)
    });
    api!(napi_throw_type_error(env, code: *const c_char, message: *const c_char) {
        check(!message.is_null(), 1)?;
        let code = if code.is_null() { ptr::null_mut() } else { env.c_text(code) };
        env.exception(env.error("TypeError", code, env.c_text(message))?);
        Ok(())
    });
    api!(napi_create_range_error(env, code: Ptr, message: Ptr, result: *mut Ptr) {
        check(env.kind(message)? == 4, 3)?;
        env.value(result, env.error("RangeError", code, message)?)
    });
    api!(napi_throw_range_error(env, code: *const c_char, message: *const c_char) {
        check(!message.is_null(), 1)?;
        let code = if code.is_null() { ptr::null_mut() } else { env.c_text(code) };
        env.exception(env.error("RangeError", code, env.c_text(message))?);
        Ok(())
    });
    api!(napi_throw(env, value: Ptr) {
        check(!value.is_null(), 1)?;
        env.exception(value);
        Ok(())
    });
    api!(napi_is_exception_pending(env, result: *mut bool) {
        check(!result.is_null(), 1)?;
        *result = !env.pending.get().is_null();
        Ok(())
    });
    api!(napi_get_and_clear_last_exception(env, result: *mut Ptr) {
        check(!result.is_null(), 1)?;
        let value = env.take_exception();
        env.value(
            result,
            if value.is_null() {
                env.undefined()
            } else {
                value
            },
        )
    });
    api!(napi_create_promise(env, deferred: *mut Ptr, result: *mut Ptr) {
        check(!deferred.is_null(), 1)?;
        let state = env.js("promise", &[])?;
        check(Self::napi_create_reference(env, state, 1, deferred) == 0, 9)?;
        env.value(result, env.js("get", &[state, env.text("promise")])?)
    });
    api!(napi_resolve_deferred(env, deferred: Ptr, value: Ptr) {
        env.settle( deferred, value, "resolve")
    });
    api!(napi_reject_deferred(env, deferred: Ptr, value: Ptr) {
        env.settle( deferred, value, "reject")
    });
    api!(napi_define_properties(
        env,
        object: Ptr,
        count: usize,
        properties: *const Property,
    ) {
        env.object(object)?;
        check(count == 0 || !properties.is_null(), 1)?;
        for index in 0..count {
            let property = &*properties.add(index);
            let key = if property.name.is_null() {
                property.key
            } else {
                env.c_text(property.name)
            };
            check(!key.is_null(), 1)?;
            let name = env.api.text(env.ctx, key);
            let value = if let Some(method) = property.method {
                env.function(&name, method, property.data)?
            } else if property.value.is_null() {
                env.undefined()
            } else {
                property.value
            };
            let getter = if let Some(callback) = property.getter {
                env.function(&name, callback, property.data)?
            } else {
                env.undefined()
            };
            let setter = if let Some(callback) = property.setter {
                env.function(&name, callback, property.data)?
            } else {
                env.undefined()
            };
            env.js(
                "define",
                &[
                    object,
                    key,
                    value,
                    getter,
                    setter,
                    env.number(property.attributes as f64),
                ],
            )?;
        }
        Ok(())
    });
    api!(napi_define_class(
        env,
        name: *const c_char,
        length: usize,
        callback: Option<Callback>,
        data: Ptr,
        count: usize,
        properties: *const Property,
        result: *mut Ptr,
    ) {
        check(count == 0 || !properties.is_null(), 1)?;
        let mut constructor = ptr::null_mut();
        check(
            Self::napi_create_function(env, name, length, callback, data, &mut constructor) == 0,
            1,
        )?;
        let prototype = env.js("get", &[constructor, env.text("prototype")])?;
        for index in 0..count {
            let property = properties.add(index);
            let object = if (*property).attributes & 1024 == 0 {
                prototype
            } else {
                constructor
            };
            let status = Self::napi_define_properties(env, object, 1, property);
            check(status == 0, status)?;
        }
        env.value(result, constructor)
    });
    api!(napi_fatal_exception(env, error: Ptr) {
        env.js("report", &[error])?;
        Ok(())
    });
    api!(napi_async_init(env, _resource: Ptr, name: Ptr, result: *mut Ptr) {
        check(!name.is_null() && !result.is_null(), 1)?;
        *result = ptr::NonNull::<u8>::dangling().as_ptr().cast();
        Ok(())
    });
    api!(napi_async_destroy(env, context: Ptr) {
        check(!context.is_null(), 1)
    });
    api!(napi_make_callback(
        env,
        _context: Ptr,
        this: Ptr,
        function: Ptr,
        argc: usize,
        argv: *const Ptr,
        result: *mut Ptr,
    ) {
        check(!this.is_null(), 1)?;
        let status = Self::napi_call_function(env, this, function, argc, argv, result);
        check(status == 0, status)
    });
    api!(napi_get_all_property_names(
        env,
        object: Ptr,
        mode: u32,
        filter: u32,
        conversion: u32,
        result: *mut Ptr,
    ) {
        env.object(object)?;
        check(mode <= 1 && conversion <= 1, 1)?;
        env.value(
            result,
            env.js(
                "all_keys",
                &[
                    object,
                    env.boolean(mode == 1),
                    env.number(filter as f64),
                    env.boolean(conversion == 1),
                ],
            )?,
        )
    });
}
