import { WebGPURenderer, Scene, PerspectiveCamera, BoxGeometry, MeshNormalMaterial, Mesh, Color } from 'three/webgpu';

(async () => {
  const renderer = new WebGPURenderer({ antialias: true });
  renderer.setSize(innerWidth, innerHeight);
  document.body.appendChild(renderer.domElement);
  await renderer.init();

  const scene = new Scene();
  scene.background = new Color('#18202c');
  const camera = new PerspectiveCamera(50, innerWidth / innerHeight, 0.1, 100);
  camera.position.z = 4;
  const cube = new Mesh(new BoxGeometry(), new MeshNormalMaterial());
  cube.rotation.set(0.4, 0.6, 0);
  scene.add(cube);
  renderer.render(scene, camera);
  globalThis.threeTestResult = 'ok';
  globalThis.threeRenderer = renderer;

  const canvas = renderer.domElement;
  canvas.style.cursor = 'grab';
  let dragging = false, previousX = 0, previousY = 0, previousTime = performance.now();
  canvas.addEventListener('pointerdown', event => {
    dragging = true;
    canvas.style.cursor = 'grabbing';
    previousX = event.clientX;
    previousY = event.clientY;
    canvas.setPointerCapture(event.pointerId);
    event.preventDefault();
  });
  canvas.addEventListener('pointermove', event => {
    if (!dragging) return;
    cube.rotation.y += (event.clientX - previousX) * 0.01;
    cube.rotation.x += (event.clientY - previousY) * 0.01;
    previousX = event.clientX;
    previousY = event.clientY;
  });
  canvas.addEventListener('lostpointercapture', () => { dragging = false; canvas.style.cursor = 'grab'; });
  renderer.setAnimationLoop(time => {
    if (!dragging) cube.rotation.y += Math.min(time - previousTime, 50) * 0.0004;
    previousTime = time;
    renderer.render(scene, camera);
  });
  addEventListener('resize', () => {
    renderer.setSize(innerWidth, innerHeight);
    camera.aspect = innerWidth / innerHeight;
    camera.updateProjectionMatrix();
  });
})().catch(error => {
  globalThis.threeTestResult = String(error) + '\n' + error.stack;
  console.error(globalThis.threeTestResult);
});
