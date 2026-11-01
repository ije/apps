const a = document.getElementById('a');
const b = document.getElementById('b');
const update = () => {
  const x = Number(a.value) | 0, y = Number(b.value) | 0;
  document.getElementById('napi').textContent = native.napiAdd(x, y);
  document.getElementById('ffi').textContent = native.ffiAdd(x, y);
  document.getElementById('callback').textContent = native.ffiDouble(x);
};
a.addEventListener('input', update);
b.addEventListener('input', update);
update();
