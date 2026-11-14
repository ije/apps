const canvas = document.querySelector('canvas')
const card = canvas.querySelector('.card')
const name = document.getElementById('name')
const waves = document.getElementById('waves')

const adapter = await navigator.gpu.requestAdapter()
const device = await adapter.requestDevice()
const context = canvas.getContext('webgpu')
const format = navigator.gpu.getPreferredCanvasFormat()
context.configure({ device, format })

// The water is a height field: slow waves, and a ring for each drop. Its slope bends the view of the
// floor and of the card below it, and gives the surface its light.
const shader = device.createShaderModule({
  code: /* wgsl */ `
    struct Scene {
      size: vec2f,
      card: vec2f,
      time: f32,
      // Device pixels for each CSS pixel.
      unit: f32,
      waves: f32,
      // The place of each drop in CSS pixels, its start time and its strength.
      drops: array<vec4f, 8>,
    }
    @group(0) @binding(0) var<uniform> scene: Scene;
    @group(0) @binding(1) var card: texture_2d<f32>;
    @group(0) @binding(2) var bilinear: sampler;

    @vertex fn vertex(@builtin(vertex_index) index: u32) -> @builtin(position) vec4f {
      // One triangle covers the canvas.
      return vec4f(f32((index << 1u) & 2u) * 2 - 1, f32(index & 2u) * 2 - 1, 0, 1);
    }

    fn height(at: vec2f) -> f32 {
      let t = scene.time;
      var h = sin(at.x * 0.031 + t * 1.3) + sin(at.y * 0.047 - t * 1.7) + sin((at.x + at.y) * 0.023 + t * 0.9);
      h *= scene.waves;
      for (var i = 0; i < 8; i++) {
        let drop = scene.drops[i];
        let age = t - drop.z;
        if (drop.w > 0 && age > 0 && age < 4) {
          // The ring moves out and fades.
          let away = length(at - drop.xy) - age * 180;
          h += drop.w * sin(away * 0.12) * exp(-abs(away) * 0.03 - age * 1.1);
        }
      }
      return h;
    }

    @fragment fn fragment(@builtin(position) position: vec4f) -> @location(0) vec4f {
      let at = position.xy / scene.unit;
      let slope = vec2f(
        height(at + vec2f(1, 0)) - height(at - vec2f(1, 0)),
        height(at + vec2f(0, 1)) - height(at - vec2f(0, 1)),
      ) / 2;
      let bent = position.xy + slope * 18 * scene.unit;
      // The floor, with bands of light where the surface focuses it.
      var color = mix(vec3f(0.03, 0.36, 0.52), vec3f(0.02, 0.60, 0.66), bent.y / scene.size.y);
      color *= 1 + 0.5 * (slope.x + slope.y);
      let origin = floor((scene.size - scene.card) / 2);
      // A soft shadow of the card on the floor.
      let reach = abs(bent - vec2f(0, 14 * scene.unit) - scene.size / 2) - scene.card / 2;
      color *= 1 - 0.35 * (1 - smoothstep(-20 * scene.unit, 24 * scene.unit, max(reach.x, reach.y)));
      let inside = (bent - origin) / scene.card;
      if (all(inside >= vec2f(0)) && all(inside <= vec2f(1))) {
        let texel = textureSampleLevel(card, bilinear, inside, 0);
        color = texel.rgb + color * (1 - texel.a);
      }
      // The sun on the slopes that face it.
      let normal = normalize(vec3f(-slope * 5, 1));
      let shine = pow(max(dot(normal, normalize(vec3f(-0.35, -0.5, 0.8))), 0), 90);
      return vec4f(color * vec3f(0.92, 0.98, 1) + shine * 0.5, 1);
    }
  `,
})
const pipeline = device.createRenderPipeline({
  layout: 'auto',
  vertex: { module: shader, entryPoint: 'vertex' },
  fragment: { module: shader, entryPoint: 'fragment', targets: [{ format }] },
})
const sampler = device.createSampler({ magFilter: 'linear', minFilter: 'linear' })
const scene = new Float32Array(40)
const uniforms = device.createBuffer({ size: scene.byteLength, usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST })

// The texture of the card. It has the size of the card in device pixels.
let texture, group, size = [0, 0]
const hold = (width, height) => {
  if (width === size[0] && height === size[1]) return
  texture?.destroy()
  size = [width, height]
  texture = device.createTexture({
    size,
    format: 'rgba8unorm',
    usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST | GPUTextureUsage.RENDER_ATTACHMENT,
  })
  group = device.createBindGroup({
    layout: pipeline.getBindGroupLayout(0),
    entries: [
      { binding: 0, resource: { buffer: uniforms } },
      { binding: 1, resource: texture.createView() },
      { binding: 2, resource: sampler },
    ],
  })
}
hold(1, 1)

// Runs when the card looks different, and after `requestPaint()`. The shader draws the card at the
// center, so the page tells the canvas that the card takes the pointer there.
canvas.onpaint = () => {
  const { width, height } = canvas.getElementImageDefaultSize(card)
  hold(Math.round(width), Math.round(height))
  device.queue.drawElementImageToTexture({ source: card }, { texture, size, premultipliedAlpha: true })
  const unit = canvas.width / canvas.clientWidth
  const [x, y] = [(canvas.width - size[0]) / 2, (canvas.height - size[1]) / 2].map(side => Math.floor(side) / unit)
  canvas.updateElementGeometry(card, { canvasTransform: new DOMMatrix().translate(x, y) })
}

// The pointer leaves drops on the water: small ones as it moves, and a large one for a press.
let next = 0, last = 0
const drop = (event, strength) => {
  scene.set([event.clientX, event.clientY, performance.now() / 1000, strength], 8 + (next++ % 8) * 4)
}
canvas.addEventListener('pointermove', event => {
  if (event.timeStamp - last < 90) return
  last = event.timeStamp
  drop(event, 0.5)
})
canvas.addEventListener('pointerdown', event => drop(event, 2.5))

const frame = now => {
  scene.set([canvas.width, canvas.height, ...size, now / 1000, canvas.width / canvas.clientWidth, waves.value / 100])
  device.queue.writeBuffer(uniforms, 0, scene)
  const encoder = device.createCommandEncoder()
  const pass = encoder.beginRenderPass({
    colorAttachments: [{ view: context.getCurrentTexture().createView(), loadOp: 'clear', storeOp: 'store' }],
  })
  pass.setPipeline(pipeline)
  pass.setBindGroup(0, group)
  pass.draw(3)
  pass.end()
  device.queue.submit([encoder.finish()])
  requestAnimationFrame(frame)
}

// A change of the card paints by itself.
name.addEventListener('input', () => (document.getElementById('who').textContent = name.value || 'water'))

// The canvas has one pixel for each pixel of the display.
new ResizeObserver(([entry]) => {
  const [box] = entry.devicePixelContentBoxSize
  canvas.width = box.inlineSize
  canvas.height = box.blockSize
  canvas.requestPaint()
}).observe(canvas)
requestAnimationFrame(frame)
