import { ipcRenderer } from '@pierre/apps';

export const send = (channel, ...args) => ipcRenderer.send(channel, ...args);
export const on = (channel, callback) => {
  ipcRenderer.on(channel, (_event, ...args) => callback(...args));
};
