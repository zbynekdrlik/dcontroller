import { spawn, type ChildProcess } from "node:child_process";
import { copyFileSync, readFileSync, writeFileSync } from "node:fs";
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
  const child: ChildProcess = spawn(
    bin,
    ["--config", device, "--ip", routed.ip, "--clock", "system", "--control", resolve(run, "control.sock")],
    { stdio: ["ignore", "inherit", "inherit"] },
  );
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
  return async () => {
    const exited = new Promise<number | null>((res) => child.once("exit", res));
    child.kill("SIGTERM");
    const code = await exited;
    if (code !== 0) throw new Error(`dstack-dev stopped with ${code}`);
  };
}
