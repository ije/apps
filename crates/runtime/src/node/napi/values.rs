use super::{State, Status, chars, check};
use crate::jsc::Ptr;
use std::{ffi::c_char, ptr};

impl State {
  unsafe fn copy_units<T>(
    &self,
    value: Ptr,
    buffer: *mut T,
    size: usize,
    result: *mut usize,
    convert: fn(u16) -> T,
  ) -> Status {
    check(!buffer.is_null() || !result.is_null(), 1)?;
    check(self.kind(value)? == 4, 3)?;
    unsafe {
      let string = (self.api.to_string)(self.ctx, value, ptr::null_mut());
      let length = (self.api.string_length)(string);
      let units = (self.api.string_chars)(string);
      let count = length.min(size.saturating_sub(1));
      if !buffer.is_null() && size > 0 {
        for index in 0..count {
          buffer.add(index).write(convert(*units.add(index)));
        }
        buffer.add(count).write(convert(0));
      }
      (self.api.string_release)(string);
      if !result.is_null() {
        *result = if buffer.is_null() {
          length
        } else if size == 0 {
          0
        } else {
          count
        };
      }
    }
    Ok(())
  }

  api!(napi_get_undefined(env, result: *mut Ptr) {
      env.value(result, env.undefined())
  });
  api!(napi_get_null(env, result: *mut Ptr) {
      env.value(result, (env.api.null)(env.ctx))
  });
  api!(napi_get_global(env, result: *mut Ptr) {
      env.value(result, (env.api.global)(env.ctx))
  });
  api!(napi_get_boolean(env, value: bool, result: *mut Ptr) {
      env.value(result, env.boolean(value))
  });
  api!(napi_create_object(env, result: *mut Ptr) {
      env.value(result, env.js("object", &[])?)
  });
  api!(napi_create_array(env, result: *mut Ptr) {
      env.value(result, env.js("array", &[env.number(0.)])?)
  });
  api!(napi_create_array_with_length(env, length: usize, result: *mut Ptr) {
      env.value(result, env.js("array", &[env.number(length as f64)])?)
  });
  api!(napi_typeof(env, value: Ptr, result: *mut u32) {
      check(!result.is_null(), 1)?;
      *result = if env.external(value)?.is_some() {
          8
      } else {
          env.kind(value)?
      };
      Ok(())
  });
  api!(napi_get_value_bool(env, value: Ptr, result: *mut bool) {
      check(!result.is_null(), 1)?;
      check(env.kind(value)? == 2, 7)?;
      *result = (env.api.to_boolean)(env.ctx, value);
      Ok(())
  });
  api!(napi_create_symbol(env, description: Ptr, result: *mut Ptr) {
      env.value(
          result,
          env.js(
              "symbol",
              &[if description.is_null() {
                  env.undefined()
              } else {
                  description
              }],
          )?,
      )
  });
  api!(napi_create_double(env, value: f64, result: *mut Ptr) {
      env.value(result, env.number(value))
  });
  api!(napi_get_value_double(env, value: Ptr, result: *mut f64) {
      check(!result.is_null(), 1)?;
      check(env.kind(value)? == 3, 6)?;
      let number = (env.api.to_number)(env.ctx, value, ptr::null_mut());
      *result = number;
      Ok(())
  });
  api!(napi_create_int32(env, value: i32, result: *mut Ptr) {
      env.value(result, env.number(value as f64))
  });
  api!(napi_get_value_int32(env, value: Ptr, result: *mut i32) {
      check(!result.is_null(), 1)?;
      check(env.kind(value)? == 3, 6)?;
      let number = (env.api.to_number)(env.ctx, value, ptr::null_mut());
      *result = number.trunc().rem_euclid(4294967296.) as u32 as i32;
      Ok(())
  });
  api!(napi_create_uint32(env, value: u32, result: *mut Ptr) {
      env.value(result, env.number(value as f64))
  });
  api!(napi_get_value_uint32(env, value: Ptr, result: *mut u32) {
      check(!result.is_null(), 1)?;
      check(env.kind(value)? == 3, 6)?;
      let number = (env.api.to_number)(env.ctx, value, ptr::null_mut());
      *result = number.trunc().rem_euclid(4294967296.) as u32;
      Ok(())
  });
  api!(napi_create_int64(env, value: i64, result: *mut Ptr) {
      env.value(result, env.number(value as f64))
  });
  api!(napi_get_value_int64(env, value: Ptr, result: *mut i64) {
      check(!result.is_null(), 1)?;
      check(env.kind(value)? == 3, 6)?;
      let number = (env.api.to_number)(env.ctx, value, ptr::null_mut());
      *result = if number.is_finite() { number as i64 } else { 0 };
      Ok(())
  });
  api!(napi_create_string_utf8(
      env,
      value: *const c_char,
      length: usize,
      result: *mut Ptr,
  ) {
      check(!value.is_null() || length == 0, 1)?;
      env.value(result, env.text(&String::from_utf8_lossy(chars(value, length))))
  });
  api!(napi_get_value_string_utf8(
      env,
      value: Ptr,
      buffer: *mut c_char,
      size: usize,
      result: *mut usize,
  ) {
      check(!buffer.is_null() || !result.is_null(), 1)?;
      check(env.kind(value)? == 4, 3)?;
      let text = env.api.text(env.ctx, value);
      let mut count = text.len().min(size.saturating_sub(1));
      while !text.is_char_boundary(count) {
          count -= 1;
      }
      if !buffer.is_null() && size > 0 {
          ptr::copy_nonoverlapping(text.as_ptr(), buffer.cast::<u8>(), count);
          *buffer.add(count) = 0;
      }
      if !result.is_null() {
          *result = if buffer.is_null() {
              text.len()
          } else if size == 0 {
              0
          } else {
              count
          };
      }
      Ok(())
  });
  api!(napi_create_string_latin1(
      env,
      value: *const c_char,
      length: usize,
      result: *mut Ptr,
  ) {
      check(!value.is_null() || length == 0, 1)?;
      let units: Vec<u16> = chars(value, length).iter().map(|b| *b as u16).collect();
      env.value(result, env.utf16(&units))
  });
  api!(napi_get_value_string_latin1(
      env,
      value: Ptr,
      buffer: *mut c_char,
      size: usize,
      result: *mut usize,
  ) {
      env.copy_units(value, buffer, size, result, |unit| unit as c_char)
  });
  api!(napi_create_string_utf16(
      env,
      value: *const u16,
      length: usize,
      result: *mut Ptr,
  ) {
      check(!value.is_null() || length == 0, 1)?;
      let length = if length == usize::MAX {
          let mut size = 0;
          while *value.add(size) != 0 {
              size += 1;
          }
          size
      } else {
          length
      };
      let units = if length == 0 { &[] } else { std::slice::from_raw_parts(value, length) };
      env.value(result, env.utf16(units))
  });
  api!(napi_get_value_string_utf16(
      env,
      value: Ptr,
      buffer: *mut u16,
      size: usize,
      result: *mut usize,
  ) {
      env.copy_units(value, buffer, size, result, |unit| unit)
  });
  api!(napi_is_array(env, value: Ptr, result: *mut bool) {
      check(!result.is_null(), 1)?;
      *result = env.truthy("is_array", &[value])?;
      Ok(())
  });
  api!(napi_is_error(env, value: Ptr, result: *mut bool) {
      check(!result.is_null(), 1)?;
      *result = env.truthy("is_error", &[value])?;
      Ok(())
  });
  api!(napi_is_promise(env, value: Ptr, result: *mut bool) {
      check(!result.is_null(), 1)?;
      *result = env.truthy("is_promise", &[value])?;
      Ok(())
  });
  api!(napi_is_date(env, value: Ptr, result: *mut bool) {
      check(!result.is_null(), 1)?;
      *result = env.truthy("is_date", &[value])?;
      Ok(())
  });
  api!(napi_coerce_to_number(env, value: Ptr, result: *mut Ptr) {
      env.value(result, env.js("number", &[value])?)
  });
  api!(napi_coerce_to_string(env, value: Ptr, result: *mut Ptr) {
      env.value(result, env.js("string", &[value])?)
  });
  api!(napi_coerce_to_object(env, value: Ptr, result: *mut Ptr) {
      env.value(result, env.js("to_object", &[value])?)
  });
  api!(napi_strict_equals(env, a: Ptr, b: Ptr, result: *mut bool) {
      check(!result.is_null(), 1)?;
      *result = env.truthy("equal", &[a, b])?;
      Ok(())
  });
  api!(napi_instanceof(env, a: Ptr, b: Ptr, result: *mut bool) {
      check(!result.is_null(), 1)?;
      *result = env.truthy("instanceof", &[a, b])?;
      Ok(())
  });
  api!(napi_create_date(env, value: f64, result: *mut Ptr) {
      env.value(result, env.js("date", &[env.number(value)])?)
  });
  api!(napi_get_date_value(env, value: Ptr, result: *mut f64) {
      check(!result.is_null(), 1)?;
      check(env.truthy("is_date", &[value])?, 18)?;
      let value = env.js("date_value", &[value])?;
      *result = (env.api.to_number)(env.ctx, value, ptr::null_mut());
      Ok(())
  });
  api!(napi_create_bigint_int64(env, value: i64, result: *mut Ptr) {
      env.value(result, env.js("bigint", &[env.text(&value.to_string())])?)
  });
  api!(napi_get_value_bigint_int64(
      env,
      value: Ptr,
      result: *mut i64,
      lossless: *mut bool,
  ) {
      check(!result.is_null() && !lossless.is_null(), 1)?;
      check(env.kind(value)? == 9, 17)?;
      let text = env.js("bigint_i64", &[value])?;
      *result = env.api.text(env.ctx, text).parse().map_err(|_| 9)?;
      *lossless = env.truthy("bigint_lossless", &[value, env.boolean(true)])?;
      Ok(())
  });
  api!(napi_create_bigint_uint64(env, value: u64, result: *mut Ptr) {
      env.value(result, env.js("bigint", &[env.text(&value.to_string())])?)
  });
  api!(napi_get_value_bigint_uint64(
      env,
      value: Ptr,
      result: *mut u64,
      lossless: *mut bool,
  ) {
      check(!result.is_null() && !lossless.is_null(), 1)?;
      check(env.kind(value)? == 9, 17)?;
      let text = env.js("bigint_u64", &[value])?;
      *result = env.api.text(env.ctx, text).parse().map_err(|_| 9)?;
      *lossless = env.truthy("bigint_lossless", &[value, env.boolean(false)])?;
      Ok(())
  });
  api!(napi_coerce_to_bool(env, value: Ptr, result: *mut Ptr) {
      env.value(result, env.boolean((env.api.to_boolean)(env.ctx, value)))
  });
  api!(napi_get_value_bigint_words(
      env,
      value: Ptr,
      sign: *mut i32,
      count: *mut usize,
      words: *mut u64,
  ) {
      check(!count.is_null(), 1)?;
      check(env.kind(value)? == 9, 17)?;
      let text = env.api.text(env.ctx, env.js("bigint_words", &[value])?);
      let mut parts = text.split(',');
      let negative = parts.next() == Some("1");
      let list: Vec<u64> = parts.filter_map(|word| word.parse().ok()).collect();
      if !sign.is_null() {
          *sign = i32::from(negative);
      }
      if !words.is_null() {
          for (index, word) in list.iter().take(*count).enumerate() {
              *words.add(index) = *word;
          }
      }
      // The count is the words the value needs, also when the buffer was shorter.
      *count = list.len();
      Ok(())
  });
  api!(napi_type_tag_object(env, object: Ptr, tag: *const [u64; 2]) {
      check(!tag.is_null(), 1)?;
      env.object(object)?;
      let existing = env.js("type_tag", &[object])?;
      check((env.api.value_type)(env.ctx, existing) == 0, 1)?;
      env.js("type_tag", &[object, env.text(&format!("{:?}", *tag))])?;
      Ok(())
  });
  api!(napi_check_object_type_tag(env, object: Ptr, tag: *const [u64; 2], result: *mut bool) {
      check(!tag.is_null() && !result.is_null(), 1)?;
      env.object(object)?;
      let existing = env.js("type_tag", &[object])?;
      *result = (env.api.value_type)(env.ctx, existing) != 0
          && env.api.text(env.ctx, existing) == format!("{:?}", *tag);
      Ok(())
  });
  api!(napi_create_bigint_words(
      env,
      sign: i32,
      count: usize,
      words: *const u64,
      result: *mut Ptr,
  ) {
      check(!result.is_null() && (count == 0 || !words.is_null()) && count <= i32::MAX as usize, 1)?;
      let words = if count == 0 { &[][..] } else { std::slice::from_raw_parts(words, count) };
      let mut hex = String::with_capacity(3 + 16 * count);
      hex.push_str("0x0");
      for word in words.iter().rev() {
          hex.push_str(&format!("{word:016x}"));
      }
      env.value(result, env.js("bigint_hex", &[env.text(&hex), env.boolean(sign != 0)])?)
  });
}
