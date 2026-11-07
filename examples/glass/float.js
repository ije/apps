import { on, send } from './preload.js';

const float = document.querySelector('#float');
on('mouse-state', (enabled) => {
  document.querySelector('#device-state').textContent = enabled ? 'On' : 'Off';
});
float.addEventListener('mouseenter', () => send('float-hover', true));
float.addEventListener('mouseleave', () => send('float-hover', false));
document.querySelector('#close').addEventListener('click', () => send('float-close'));
send('float-ready');
