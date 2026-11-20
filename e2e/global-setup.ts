import { spawn, type ChildProcess } from "node:child_process";
import { copyFileSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { createServer, type Server } from "node:http";
import { DSYNC, VIDEO, run, root, routed } from "./env";

// The stand-in dsync service node: its status, as dsync's contract names
// the fields (the ones it says are no health, `offset_ns` and `settled`,
// included: the server must not read them). `schema` is the contract's
// version: absent on an older dsync, 2 on one this dcontroller does not read.
// camera-box's node list and its two nodes (#124 phase 1), as its draft
// contract names the fields (camera-box#1540). The OBS node's input loses
// one frame per read from its fourth read on.
function startVideo(): Promise<Server[]> {
  const listen = (at: string, answer: (url: string) => object | null): Promise<Server> => {
    const [host, port] = at.split(":");
    return new Promise((ok, fail) => {
      const server = createServer((req, res) => {
        const body = answer(req.url ?? "");
        if (body === null) {
          res.writeHead(404).end();
          return;
        }
        res.writeHead(200, { "content-type": "application/json" });
        res.end(JSON.stringify(body));
      });
      server.once("error", fail);
      server.listen(Number(port), host, () => ok(server));
    });
  };
  let reads = 0;
  const list = listen(VIDEO.list, (url) =>
    url === "/video-nodes.json"
      ? {
          schema: 1,
          venue: "e2e",
          nodes: [
            { name: VIDEO.cam, kind: "cambox", url: `http://${VIDEO.nodes}/cam.json` },
            { name: VIDEO.obs, kind: "obs", url: `http://${VIDEO.nodes}/obs.json` },
          ],
        }
      : null,
  );
  const nodes = listen(VIDEO.nodes, (url) => {
    const head = { schema: 1, read_at: new Date().toISOString(), run_start: "2026-11-20T00:00:00Z", run_id: "e2e:1" };
    if (url === "/cam.json") return { ...head, node: VIDEO.cam, kind: "cambox", connections: [] };
    if (url !== "/obs.json") return null;
    reads += 1;
    return {
      ...head,
      node: VIDEO.obs,
      kind: "obs",
      connections: [
        {
          id: VIDEO.conn,
          source: VIDEO.cam,
          sink: VIDEO.obs,
          sink_input: "JXS cam-e2e src",
          kind: "jxs",
          direction: "in",
          latency_ms: 40 + (reads % 3),
          frames_received: reads * 500,
          frames_lost: Math.max(0, reads - 3),
          frames_duplicated: 0,
          frames_skipped: 0,
          slices_lost: 0,
          slices_concealed: 0,
          relocks: 0,
          decoder_opens: 1,
          counters_since: "2026-11-20T00:00:02Z",
        },
      ],
    };
  });
  return Promise.all([list, nodes]);
}

function startDsync(at: string, schema: number | null): Promise<Server> {
  const status = {
    ...(schema === null ? {} : { schema }),
    version: "0.9.4",
    mode: "LOCK",
    ptp_phase_error_us: 3.25,
    ptp_phase_locked: true,
    last_ptp_rx_age_s: 0.4,
    ptp_rx_pps: 8,
    gm_uuid: [0, 29, 193, 170, 187, 204],
    gm_source_ip: "10.199.0.200",
    drift_ppm: -1.75,
    fallback_role: "none",
    clock_alarm: { active: false, since: null, reason: "" },
    offset_ns: 123456789012,
    settled: true,
  };
  const [host, port] = at.split(":");
  return new Promise((ok, fail) => {
    const server = createServer((req, res) => {
      if (req.url === "/status") {
        res.writeHead(200, { "content-type": "application/json" });
        res.end(JSON.stringify(status));
      } else {
        res.writeHead(404).end();
      }
    });
    server.once("error", fail);
    server.listen(Number(port), host, () => ok(server));
  });
}

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
  const dsync = [
    await startDsync(DSYNC.up, 1),
    await startDsync(DSYNC.old, null),
    await startDsync(DSYNC.newer, 2),
  ];
  const video = await startVideo();
  return async () => {
    for (const s of [...dsync, ...video]) await new Promise((done) => s.close(done));
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
