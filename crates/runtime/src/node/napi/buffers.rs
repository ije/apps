use super::{
    State, check,
    objects::{Finalize, Finalizer, Kind, release_holder},
};
use crate::jsc::Ptr;
use std::ptr;

unsafe extern "C" fn free_buffer(_: Ptr, context: Ptr) {
    unsafe {
        drop(Box::from_raw(context.cast::<Vec<u8>>()));
    }
}

impl State {
    api!(napi_is_dataview(env, value: Ptr, result: *mut bool) {
        check(!result.is_null(), 1)?;
        *result = env.truthy("is_dataview", &[value])?;
        Ok(())
    });
    api!(napi_create_arraybuffer(env, length: usize, data: *mut Ptr, result: *mut Ptr) {
        check(!result.is_null(), 1)?;
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(length).map_err(|_| 9)?;
        bytes.resize(length, 0);
        let mut bytes = Box::new(bytes);
        let pointer = bytes.as_mut_ptr().cast();
        let mut exception = ptr::null_mut();
        let value = (env.api.make_buffer)(
            env.ctx,
            pointer,
            length,
            Some(free_buffer),
            Box::into_raw(bytes).cast(),
            &mut exception,
        );
        if !exception.is_null() {
            env.exception(exception);
            return Err(10);
        }
        if !data.is_null() {
            *data = pointer;
        }
        env.value(result, value)
    });
    api!(napi_create_external_arraybuffer(
        env,
        data: Ptr,
        length: usize,
        callback: Option<Finalize>,
        hint: Ptr,
        result: *mut Ptr,
    ) {
        check((!data.is_null() || length == 0) && !result.is_null(), 1)?;
        // The finalizer follows the bytes, not the wrapper, because ArrayBuffer.transfer() moves them.
        let (id, holder) = env.objects.holder();
        let mut exception = ptr::null_mut();
        let value = (env.api.make_buffer)(env.ctx, data, length, Some(release_holder), holder.cast(), &mut exception);
        if !exception.is_null() {
            env.exception(exception);
            return Err(10);
        }
        if let Some(callback) = callback {
            env.objects.insert(id, ptr::null_mut(), Kind::Finalizer, Some(Finalizer { callback, data, hint }));
        }
        env.value(result, value)
    });
    api!(napi_is_arraybuffer(env, value: Ptr, result: *mut bool) {
        check(!value.is_null() && !result.is_null(), 1)?;
        *result = (env.api.typed_kind)(env.ctx, value, ptr::null_mut()) == 9;
        Ok(())
    });
    api!(napi_get_arraybuffer_info(
        env,
        value: Ptr,
        data: *mut Ptr,
        length: *mut usize,
    ) {
        check(!value.is_null(), 1)?;
        check(
            (env.api.typed_kind)(env.ctx, value, ptr::null_mut()) == 9,
            19,
        )?;
        if !data.is_null() {
            *data = (env.api.buffer_bytes)(env.ctx, value, ptr::null_mut());
        }
        if !length.is_null() {
            *length = (env.api.buffer_length)(env.ctx, value, ptr::null_mut());
        }
        Ok(())
    });
    api!(napi_create_typedarray(
        env,
        kind: u32,
        length: usize,
        buffer: Ptr,
        offset: usize,
        result: *mut Ptr,
    ) {
        check(kind <= 10, 1)?;
        env.value(
            result,
            env.js(
                "typed",
                &[
                    env.number(kind as f64),
                    buffer,
                    env.number(offset as f64),
                    env.number(length as f64),
                ],
            )?,
        )
    });
    api!(napi_is_typedarray(env, value: Ptr, result: *mut bool) {
        check(!value.is_null() && !result.is_null(), 1)?;
        let kind = (env.api.typed_kind)(env.ctx, value, ptr::null_mut());
        *result = kind <= 8 || kind == 11 || kind == 12;
        Ok(())
    });
    api!(napi_get_typedarray_info(
        env,
        value: Ptr,
        kind: *mut u32,
        length: *mut usize,
        data: *mut Ptr,
        buffer: *mut Ptr,
        offset: *mut usize,
    ) {
        check(!value.is_null(), 1)?;
        let native = (env.api.typed_kind)(env.ctx, value, ptr::null_mut());
        let (ty, size) = match native {
            0 => (0, 1),
            1 => (3, 2),
            2 => (5, 4),
            3 => (1, 1),
            4 => (2, 1),
            5 => (4, 2),
            6 => (6, 4),
            7 => (7, 4),
            8 => (8, 8),
            11 => (9, 8),
            12 => (10, 8),
            _ => return Err(1),
        };
        let byte_offset = (env.api.typed_offset)(env.ctx, value, ptr::null_mut());
        if !kind.is_null() {
            *kind = ty;
        }
        if !length.is_null() {
            *length = (env.api.typed_length)(env.ctx, value, ptr::null_mut()) / size;
        }
        if !data.is_null() {
            *data = (env.api.typed_bytes)(env.ctx, value, ptr::null_mut())
                .cast::<u8>()
                .wrapping_add(byte_offset)
                .cast();
        }
        if !offset.is_null() {
            *offset = byte_offset;
        }
        if !buffer.is_null() {
            *buffer = env.keep((env.api.typed_buffer)(env.ctx, value, ptr::null_mut()));
        }
        Ok(())
    });
    api!(napi_create_buffer(env, length: usize, data: *mut Ptr, result: *mut Ptr) {
        let mut buffer = ptr::null_mut();
        let status = Self::napi_create_arraybuffer(env, length, data, &mut buffer);
        check(status == 0, status)?;
        env.value(result, env.js("buffer", &[buffer])?)
    });
    api!(napi_create_buffer_copy(
        env,
        length: usize,
        data: Ptr,
        copied: *mut Ptr,
        result: *mut Ptr,
    ) {
        check(!data.is_null() || length == 0, 1)?;
        let mut pointer = ptr::null_mut();
        let status = Self::napi_create_buffer(env, length, &mut pointer, result);
        check(status == 0, status)?;
        if length > 0 {
            ptr::copy_nonoverlapping(data.cast::<u8>(), pointer.cast::<u8>(), length);
        }
        if !copied.is_null() {
            *copied = pointer;
        }
        Ok(())
    });
    api!(napi_create_external_buffer(
        env,
        length: usize,
        data: Ptr,
        callback: Option<Finalize>,
        hint: Ptr,
        result: *mut Ptr,
    ) {
        let mut buffer = ptr::null_mut();
        let status = Self::napi_create_external_arraybuffer(env, data, length, callback, hint, &mut buffer);
        check(status == 0, status)?;
        env.value(result, env.js("buffer", &[buffer])?)
    });
    api!(napi_is_buffer(env, value: Ptr, result: *mut bool) {
        check(!value.is_null() && !result.is_null(), 1)?;
        *result = (env.api.typed_kind)(env.ctx, value, ptr::null_mut()) == 3;
        Ok(())
    });
    api!(napi_get_buffer_info(env, value: Ptr, data: *mut Ptr, length: *mut usize) {
        check(!value.is_null(), 1)?;
        check(
            (env.api.typed_kind)(env.ctx, value, ptr::null_mut()) == 3,
            1,
        )?;
        let status = Self::napi_get_typedarray_info(
            env,
            value,
            ptr::null_mut(),
            length,
            data,
            ptr::null_mut(),
            ptr::null_mut(),
        );
        check(status == 0, status)
    });
    api!(napi_create_dataview(
        env,
        length: usize,
        buffer: Ptr,
        offset: usize,
        result: *mut Ptr,
    ) {
        env.value(
            result,
            env.js(
                "dataview",
                &[buffer, env.number(offset as f64), env.number(length as f64)],
            )?,
        )
    });
    api!(napi_get_dataview_info(
        env,
        value: Ptr,
        length: *mut usize,
        data: *mut Ptr,
        buffer: *mut Ptr,
        offset: *mut usize,
    ) {
        check(env.truthy("is_dataview", &[value])?, 1)?;
        let array = env.js("get", &[value, env.text("buffer")])?;
        check((env.api.typed_kind)(env.ctx, array, ptr::null_mut()) == 9, 1)?;
        let start = env.js("get", &[value, env.text("byteOffset")])?;
        let start = (env.api.to_number)(env.ctx, start, ptr::null_mut()) as usize;
        let size = env.js("get", &[value, env.text("byteLength")])?;
        let size = (env.api.to_number)(env.ctx, size, ptr::null_mut()) as usize;
        check(
            start.checked_add(size).is_some_and(|end| {
                end <= (env.api.buffer_length)(env.ctx, array, ptr::null_mut())
            }),
            1,
        )?;
        if !buffer.is_null() {
            *buffer = array;
        }
        if !offset.is_null() {
            *offset = start;
        }
        if !data.is_null() {
            *data = (env.api.buffer_bytes)(env.ctx, array, ptr::null_mut())
                .cast::<u8>()
                .wrapping_add(start)
                .cast();
        }
        if !length.is_null() {
            *length = size;
        }
        Ok(())
    });
    api!(napi_detach_arraybuffer(env, value: Ptr) {
        check(!value.is_null(), 1)?;
        check((env.api.typed_kind)(env.ctx, value, ptr::null_mut()) == 9, 19)?;
        check(env.truthy("detach", &[value])?, 20)
    });
    api!(napi_is_detached_arraybuffer(env, value: Ptr, result: *mut bool) {
        check(!value.is_null() && !result.is_null(), 1)?;
        *result = (env.api.typed_kind)(env.ctx, value, ptr::null_mut()) == 9
            && env.truthy("get", &[value, env.text("detached")])?;
        Ok(())
    });
}
