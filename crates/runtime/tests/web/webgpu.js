(async () => {
  const adapter = await navigator.gpu.requestAdapter();
  if (!adapter) throw Error('No native GPU adapter');
  const device = await adapter.requestDevice();
  globalThis.canvas = document.getElementById('canvas');
  globalThis.context = canvas.getContext('webgpu');
  context.configure({ device, format: navigator.gpu.getPreferredCanvasFormat() });
  const shader = device.createShaderModule({ code: `
    @vertex fn vs(@builtin(vertex_index) index: u32) -> @builtin(position) vec4f {
      var positions = array<vec2f, 3>(vec2f(0, 0.8), vec2f(-0.8, -0.8), vec2f(0.8, -0.8));
      return vec4f(positions[index], 0, 1);
    }
    @fragment fn fs() -> @location(0) vec4f { return vec4f(1, 0, 0, 1); }
  ` });
  const pipeline = await device.createRenderPipelineAsync({ layout: 'auto', vertex: { module: shader, entryPoint: 'vs' }, fragment: { module: shader, entryPoint: 'fs', targets: [{ format: navigator.gpu.getPreferredCanvasFormat() }] } });
  const encoder = device.createCommandEncoder();
  const pass = encoder.beginRenderPass({ colorAttachments: [{ view: context.getCurrentTexture().createView(), clearValue: [0, 0, 0, 1], loadOp: 'clear', storeOp: 'store' }] });
  pass.setPipeline(pipeline); pass.draw(3); pass.end();
  const command = encoder.finish();
  device.queue.submit([command]);
  let reused = false;
  try { device.queue.submit([command]); } catch { reused = true; }
  if (!reused) throw Error('Command buffer reused');
  const buffer = device.createBuffer({ size: 16, usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_SRC, mappedAtCreation: true });
  new Uint32Array(buffer.getMappedRange()).set([1, 2, 3, 4]);
  buffer.unmap();
  const compute = device.createComputePipeline({ layout: 'auto', compute: { module: device.createShaderModule({ code: `
    @group(0) @binding(0) var<storage, read_write> values: array<u32>;
    @compute @workgroup_size(1) fn main(@builtin(global_invocation_id) id: vec3u) { values[id.x] *= 3u; }
  ` }), entryPoint: 'main' } });
  const group = device.createBindGroup({ layout: compute.getBindGroupLayout(0), entries: [{ binding: 0, resource: { buffer } }] });
  const readback = device.createBuffer({ size: 16, usage: GPUBufferUsage.COPY_DST | GPUBufferUsage.MAP_READ });
  const commands = device.createCommandEncoder();
  const computePass = commands.beginComputePass();
  computePass.setPipeline(compute); computePass.setBindGroup(0, group); computePass.dispatchWorkgroups(4); computePass.end();
  commands.copyBufferToBuffer(buffer, 0, readback, 0, 16);
  device.queue.submit([commands.finish()]);
  await readback.mapAsync(GPUMapMode.READ);
  if (Array.from(new Uint32Array(readback.getMappedRange())).join() !== '3,6,9,12') throw Error('Compute or mapping failed');
  readback.unmap(); buffer.destroy(); readback.destroy();
  const upload = device.createBuffer({ size: 8, usage: GPUBufferUsage.COPY_DST | GPUBufferUsage.COPY_SRC });
  const data = new Uint32Array([99, 12, 34, 99]);
  device.queue.writeBuffer(upload, 0, data.subarray(1, 3), 1, 1);
  const output = device.createBuffer({ size: 8, usage: GPUBufferUsage.COPY_DST | GPUBufferUsage.MAP_READ });
  const copy = device.createCommandEncoder(); copy.copyBufferToBuffer(upload, 0, output, 0, 8); device.queue.submit([copy.finish()]);
  await output.mapAsync(GPUMapMode.READ);
  if (new Uint32Array(output.getMappedRange())[0] !== 34) throw Error('Typed array offsets are wrong');
  output.unmap(); upload.destroy(); output.destroy();
  device.pushErrorScope('validation');
  device.createBuffer({ size: 4, usage: 0 });
  const error = await device.popErrorScope();
  if (!(error instanceof GPUValidationError)) throw Error('Missing validation error');
  device.pushErrorScope('validation');
  const invalidShader = device.createShaderModule({ code: 'invalid WGSL' });
  if (!(await device.popErrorScope() instanceof GPUValidationError)) throw Error('Missing shader validation error');
  if (!(await invalidShader.getCompilationInfo()).messages.some(m => m.type === 'error')) throw Error('Missing shader diagnostics');
  const panicked = error => { if (error.message === 'Host call failed') throw error; };
  const unmappable = device.createBuffer({ size: 16, usage: GPUBufferUsage.COPY_DST });
  for (let i = 0; i < 2; i++) { device.pushErrorScope('validation'); await unmappable.mapAsync(GPUMapMode.READ).catch(panicked); await device.popErrorScope(); }
  device.pushErrorScope('validation');
  const invalid = device.createBuffer({ size: 16, usage: 0, mappedAtCreation: true });
  new Uint8Array(invalid.getMappedRange()).fill(1); invalid.unmap();
  if (!(await device.popErrorScope())) throw Error('Missing invalid buffer error');
  const twice = device.createBuffer({ size: 16, usage: GPUBufferUsage.MAP_READ | GPUBufferUsage.COPY_DST });
  device.queue.writeBuffer(twice, 0, new Uint32Array([7, 8, 9, 10]));
  await twice.mapAsync(GPUMapMode.READ);
  let rejected = 0;
  try { device._call('mapBuffer', [twice, 1, 0, 16]); } catch (error) { panicked(error); rejected++; }
  try { device._call('unmapBuffer', [twice, [[0, 'AAAA']]]); } catch (error) { panicked(error); rejected++; }
  const values = new Uint32Array(twice.getMappedRange());
  if (rejected !== 2 || values[3] !== 10) throw Error('Invalid mapping was accepted');
  twice.unmap();
  const broken = document.createElement('canvas'); document.body.append(broken);
  broken.getContext('webgpu').configure({ device, format: 'bgra8unorm', usage: GPUTextureUsage.STORAGE_BINDING });
  try { broken.getContext('webgpu').getCurrentTexture(); } catch (error) { panicked(error); }
  globalThis.brokenCanvas = broken._id;
  await device.queue.onSubmittedWorkDone();
  globalThis.gpuTestResult = 'ok';
})().catch(error => { globalThis.gpuTestResult = String(error) + '\n' + error.stack; });
