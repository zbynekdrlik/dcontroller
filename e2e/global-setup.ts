import { spawn, type ChildProcess } from "node:child_process";
import { copyFileSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { run, root, routed } from "./env";

// A real Dante device (dstack's dstack-dev) on the routed address, so
// the server discovers and reads it like any device on the Dante network.
export default async function globalSetup() {
  const bin = resolve(root, "target/debug/dstack-dev");
  const device = resolve(run, "device.toml");
  const state = resolve(run, "state.toml");
  copyFileSync(resolve(__dirname, "fixtures/state.toml"), state);
  writeFileSync(
    device,
    readFileSync(resolve(__dirname, "fixtures/device.toml"), "utf8") + `state_file = "${state}"\n`,
  );
  // The device's local status endpoint (dtray's, on 127.0.0.1) takes a
  // free port and logs it: a spec asks it as a local client (#97).
  const portFile = resolve(run, "status_port");
  rmSync(portFile, { force: true });
  const child: ChildProcess = spawn(
    bin,
    ["--config", device, "--ip", routed.ip, "--clock", "system", "--control", resolve(run, "control.sock")],
    { stdio: ["ignore", "pipe", "pipe"] },
  );
  for (const [from, to] of [
    [child.stdout, process.stdout],
    [child.stderr, process.stderr],
  ] as const) {
    from?.on("data", (chunk: Buffer) => {
      to.write(chunk);
      const port = /local status endpoint on 127\.0\.0\.1.*port=(\d+)/.exec(chunk.toString())?.[1];
      if (port) writeFileSync(portFile, port);
    });
  }
  const failed = new Promise<never>((_, reject) => {
    child.once("error", reject);
    child.once("exit", (code) => reject(new Error(`dstack-dev exited with ${code}`)));
  });
  // Up once it answers on its control socket.
  const deadline = Date.now() + 20_000;
  for (;;) {
    const ok = await Promise.race([
      new Promise<boolean>((res) => {
        const c = spawn(bin, ["ctl", "--socket", resolve(run, "control.sock"), "status"], { stdio: "ignore" });
        c.once("exit", (code) => res(code === 0));
      }),
      failed,
    ]);
    if (ok) break;
    if (Date.now() > deadline) {
      child.kill("SIGTERM");
      throw new Error("dstack-dev did not come up in 20 s");
    }
    await new Promise((r) => setTimeout(r, 250));
  }
  child.removeAllListeners("exit");
  const bridge = await startBridge();
  return async () => {
    for (const [c, what] of [
      [child, "dstack-dev"],
      [bridge, "bridge_device"],
    ] as const) {
      const exited = new Promise<number | null>((res) => c.once("exit", res));
      c.kill("SIGTERM");
      const code = await exited;
      if (code !== 0) throw new Error(`${what} stopped with ${code}`);
    }
  };
}

// The bridge test device (#108): a real dstack device holding bridge
// figures (crates/server/examples/bridge_device.rs), on the namespace's
// second address (isolated.sh), up once it says it serves.
async function startBridge(): Promise<ChildProcess> {
  const bin = resolve(root, "target/debug/examples/bridge_device");
  const config = resolve(run, "bridge-device.toml");
  writeFileSync(
    config,
    readFileSync(resolve(__dirname, "fixtures/bridge-device.toml"), "utf8") +
      `state_file = "${resolve(run, "bridge-state.toml")}"\n`,
  );
  rmSync(resolve(run, "bridge-state.toml"), { force: true });
  const child = spawn(bin, [config, BRIDGE_IP, resolve(run, "bridge-control.sock")], {
    stdio: ["ignore", "pipe", "pipe"],
  });
  child.stderr?.on("data", (c: Buffer) => process.stderr.write(c));
  await new Promise<void>((ok, fail) => {
    const timer = setTimeout(() => fail(new Error("bridge_device did not come up in 20 s")), 20_000);
    child.stdout?.on("data", (c: Buffer) => {
      process.stdout.write(c);
      if (c.toString().includes("bridge_device: serving")) {
        clearTimeout(timer);
        ok();
      }
    });
    child.once("error", fail);
    child.once("exit", (code) => fail(new Error(`bridge_device exited with ${code}`)));
  });
  child.removeAllListeners("exit");
  return child;
}

export const BRIDGE_IP = "10.199.0.2";
