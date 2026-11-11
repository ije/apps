globalThis.__appsPty = call => {
  const sessions=new Map();
  const spawn=(file,args=[],options={})=>{
    // Like node-pty, a terminal starts where the app is now, with the environment it has now.
    const env=Object.fromEntries(Object.entries(options.env??process.env).filter(([,value])=>value!==undefined).map(([name,value])=>[name,String(value)]));
    const state=call('pty.spawn',file,args,{...options,cwd:options.cwd??process.cwd(),env}),listeners={data:new Set(),exit:new Set()};
    const terminal={pid:state.pid,cols:options.cols??80,rows:options.rows??24,process:file,
      write:data=>call('pty.write',state.id,data),
      resize(cols,rows){call('pty.resize',state.id,cols,rows);this.cols=cols;this.rows=rows;},
      kill:signal=>call('pty.kill',state.id,signal??'SIGHUP'),
      onData(fn){listeners.data.add(fn);return{dispose:()=>listeners.data.delete(fn)};},
      onExit(fn){listeners.exit.add(fn);return{dispose:()=>listeners.exit.delete(fn)};},
    };sessions.set(state.id,listeners);return terminal;
  };
  const poll=()=>{if(!sessions.size)return;for(const event of call('pty.poll')){const listeners=sessions.get(event.id);if(!listeners)continue;if(event.type==='data')for(const fn of listeners.data)fn(event.data);else if(event.type==='close')sessions.delete(event.id);else for(const fn of listeners.exit)fn({exitCode:event.exitCode,signal:event.signal});}};
  poll.pending=()=>sessions.size>0;return {api:{spawn},poll};
};
