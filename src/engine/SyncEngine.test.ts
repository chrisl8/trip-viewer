import { beforeEach, describe, expect, it } from "vitest";
import { useStore } from "../state/store";
import { SyncEngine } from "./SyncEngine";

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

class FakeVideo extends EventTarget {
  paused = true;
  ended = false;
  readyState = 4;
  duration = 300;
  playbackRate = 1;
  seeking = false;
  playCalls = 0;
  pauseCalls = 0;
  seekWrites = 0;
  playDelayMs = 0;
  seekDelayMs = 0;
  rejectPlay = false;
  muted = false;
  defaultMuted = false;
  volume = 1;
  loadCalls = 0;
  private time = 0;

  constructor(options?: {
    currentTime?: number;
    paused?: boolean;
    playDelayMs?: number;
    seekDelayMs?: number;
    rejectPlay?: boolean;
  }) {
    super();
    this.time = options?.currentTime ?? 0;
    this.paused = options?.paused ?? true;
    this.playDelayMs = options?.playDelayMs ?? 0;
    this.seekDelayMs = options?.seekDelayMs ?? 0;
    this.rejectPlay = options?.rejectPlay ?? false;
  }

  get currentTime(): number {
    return this.time;
  }

  set currentTime(value: number) {
    this.time = value;
    this.seekWrites += 1;
    this.seeking = true;
    setTimeout(() => {
      this.seeking = false;
      this.dispatchEvent(new Event("seeked"));
    }, this.seekDelayMs);
  }

  play(): Promise<void> {
    this.playCalls += 1;
    return new Promise((resolve, reject) => {
      setTimeout(() => {
        if (this.rejectPlay) {
          reject(new Error("play failed"));
          return;
        }
        this.paused = false;
        this.dispatchEvent(new Event("playing"));
        resolve();
      }, this.playDelayMs);
    });
  }

  pause(): void {
    this.pauseCalls += 1;
    this.paused = true;
    this.dispatchEvent(new Event("pause"));
  }

  load(): void {
    this.loadCalls += 1;
    this.paused = true;
    this.seeking = false;
    setTimeout(() => this.dispatchEvent(new Event("loadedmetadata")), 0);
  }

  asMedia(): HTMLVideoElement {
    return this as unknown as HTMLVideoElement;
  }
}

function engineFor(master: FakeVideo, slaves: FakeVideo[]): SyncEngine {
  return new SyncEngine(
    master.asMedia(),
    slaves.map((s) => s.asMedia()),
    slaves.map((_, i) => `Slave ${i + 1}`),
  );
}

beforeEach(() => {
  useStore.setState({
    isPlaying: false,
    speed: 1,
    error: null,
    gappedChannels: {},
  });
});

describe("SyncEngine transport barriers", () => {
  it("rate change while playing re-seeks master and slaves, then resumes aligned", async () => {
    const master = new FakeVideo({ currentTime: 12, seekDelayMs: 1 });
    const a = new FakeVideo({ currentTime: 3, seekDelayMs: 2 });
    const b = new FakeVideo({ currentTime: 18, seekDelayMs: 1 });
    const engine = engineFor(master, [a, b]);

    await engine.play();
    a.currentTime = 4;
    b.currentTime = 20;
    await sleep(3);

    useStore.getState().setSpeed(2);
    const masterSeekWritesBefore = master.seekWrites;
    await engine.setSpeed(2);

    expect(master.seekWrites).toBeGreaterThan(masterSeekWritesBefore);
    expect(master.playbackRate).toBe(2);
    expect(a.playbackRate).toBe(2);
    expect(b.playbackRate).toBe(2);
    expect(master.muted).toBe(true);
    expect(a.muted).toBe(true);
    expect(b.muted).toBe(true);
    expect(a.currentTime).toBeCloseTo(master.currentTime, 6);
    expect(b.currentTime).toBeCloseTo(master.currentTime, 6);
    expect(master.paused).toBe(false);
    expect(a.paused).toBe(false);
    expect(b.paused).toBe(false);
    expect(useStore.getState().isPlaying).toBe(true);

    engine.dispose();
  });

  it("rate change while paused stays paused", async () => {
    const master = new FakeVideo({ currentTime: 5 });
    const slave = new FakeVideo({ currentTime: 1 });
    const engine = engineFor(master, [slave]);

    useStore.getState().setSpeed(0.5);
    await engine.setSpeed(0.5);

    expect(master.paused).toBe(true);
    expect(slave.paused).toBe(true);
    expect(slave.currentTime).toBeCloseTo(master.currentTime, 6);
    expect(useStore.getState().isPlaying).toBe(false);

    engine.dispose();
  });


  it("returning from stretched playback to 1x rebuilds the master audio pipeline", async () => {
    const master = new FakeVideo({ currentTime: 9, seekDelayMs: 1 });
    const slave = new FakeVideo({ currentTime: 2, seekDelayMs: 1 });
    const engine = engineFor(master, [slave]);
    await engine.play();

    useStore.getState().setSpeed(2);
    await engine.setSpeed(2);
    expect(master.muted).toBe(true);
    expect(master.volume).toBe(0);

    useStore.getState().setSpeed(1);
    await engine.setSpeed(1);

    expect(master.loadCalls).toBe(1);
    expect(master.playbackRate).toBe(1);
    expect(master.muted).toBe(false);
    expect(master.volume).toBe(1);
    expect(slave.muted).toBe(true);
    expect(slave.currentTime).toBeCloseTo(master.currentTime, 6);
    expect(useStore.getState().isPlaying).toBe(true);

    engine.dispose();
  });

  it("Pause during a slow speed transition cancels its pending restart", async () => {
    const master = new FakeVideo({ currentTime: 8, seekDelayMs: 1 });
    const slave = new FakeVideo({ currentTime: 2, seekDelayMs: 1 });
    const engine = engineFor(master, [slave]);
    await engine.play();

    master.seekDelayMs = 20;
    slave.seekDelayMs = 20;
    useStore.getState().setSpeed(4);
    const changing = engine.setSpeed(4);
    await sleep(2);
    engine.pause();
    await changing;
    await sleep(25);

    expect(master.paused).toBe(true);
    expect(slave.paused).toBe(true);
    expect(useStore.getState().isPlaying).toBe(false);

    engine.dispose();
  });

  it("rapid rate changes allow only the newest transition to resume", async () => {
    const master = new FakeVideo({ currentTime: 7, seekDelayMs: 1 });
    const slave = new FakeVideo({ currentTime: 3, seekDelayMs: 1 });
    const engine = engineFor(master, [slave]);
    await engine.play();

    master.seekDelayMs = 15;
    slave.seekDelayMs = 15;
    useStore.getState().setSpeed(2);
    const first = engine.setSpeed(2);
    await sleep(2);
    useStore.getState().setSpeed(4);
    const second = engine.setSpeed(4);
    await Promise.all([first, second]);

    expect(master.playbackRate).toBe(4);
    expect(slave.playbackRate).toBe(4);
    expect(master.paused).toBe(false);
    expect(slave.paused).toBe(false);
    expect(useStore.getState().isPlaying).toBe(true);

    engine.dispose();
  });

  it("toggle pauses real playback even if the store flag became stale", async () => {
    const master = new FakeVideo();
    const slave = new FakeVideo();
    const engine = engineFor(master, [slave]);
    await engine.play();

    useStore.getState().setIsPlaying(false);
    engine.togglePlayback();

    expect(master.paused).toBe(true);
    expect(slave.paused).toBe(true);
    expect(useStore.getState().isPlaying).toBe(false);

    engine.dispose();
  });

  it("does not wait for a slow slave play promise before marking playback live", async () => {
    const master = new FakeVideo({ playDelayMs: 1, seekDelayMs: 1 });
    const slave = new FakeVideo({ playDelayMs: 80, seekDelayMs: 1 });
    const engine = engineFor(master, [slave]);

    await engine.play();

    expect(master.paused).toBe(false);
    expect(slave.playCalls).toBe(1);
    expect(useStore.getState().isPlaying).toBe(true);

    engine.pause();
    engine.dispose();
  });

  it("a rejected slave play does not stop the master or healthy cameras", async () => {
    const master = new FakeVideo({ seekDelayMs: 1 });
    const failed = new FakeVideo({ rejectPlay: true, seekDelayMs: 1 });
    const healthy = new FakeVideo({ seekDelayMs: 1 });
    const engine = engineFor(master, [failed, healthy]);
    await engine.play();
    await sleep(2);
    expect(master.paused).toBe(false);
    expect(healthy.paused).toBe(false);
    expect(failed.paused).toBe(true);
    expect(useStore.getState().isPlaying).toBe(true);
    expect(useStore.getState().error).toBeNull();
    engine.pause();
    engine.dispose();
  });

  it("master play failure leaves every channel paused and UI on Play", async () => {
    const master = new FakeVideo({ rejectPlay: true, seekDelayMs: 1 });
    const slave = new FakeVideo({ seekDelayMs: 1 });
    const engine = engineFor(master, [slave]);

    await engine.play();
    await sleep(2);

    expect(master.paused).toBe(true);
    expect(slave.paused).toBe(true);
    expect(useStore.getState().isPlaying).toBe(false);
    expect(useStore.getState().error).toContain("play failed");

    engine.dispose();
  });

  it("treats an active seek as a pause-seek-resume barrier", async () => {
    const master = new FakeVideo({ currentTime: 5, seekDelayMs: 12 });
    const a = new FakeVideo({ currentTime: 2, seekDelayMs: 10 });
    const b = new FakeVideo({ currentTime: 8, seekDelayMs: 8 });
    const engine = engineFor(master, [a, b]);

    await engine.play();
    const seeking = engine.seek(42);
    await sleep(2);

    expect(master.paused).toBe(true);
    expect(a.paused).toBe(true);
    expect(b.paused).toBe(true);

    await seeking;

    expect(master.currentTime).toBeCloseTo(42, 6);
    expect(a.currentTime).toBeCloseTo(42, 6);
    expect(b.currentTime).toBeCloseTo(42, 6);
    expect(master.paused).toBe(false);
    expect(a.paused).toBe(false);
    expect(b.paused).toBe(false);
    expect(useStore.getState().isPlaying).toBe(true);

    engine.dispose();
  });

  it("can seek without resuming for segment handoff", async () => {
    const master = new FakeVideo({ currentTime: 4, seekDelayMs: 2 });
    const slave = new FakeVideo({ currentTime: 1, seekDelayMs: 2 });
    const engine = engineFor(master, [slave]);

    await engine.play();
    await engine.seek(17, { resume: false });

    expect(master.currentTime).toBeCloseTo(17, 6);
    expect(slave.currentTime).toBeCloseTo(17, 6);
    expect(master.paused).toBe(true);
    expect(slave.paused).toBe(true);
    expect(useStore.getState().isPlaying).toBe(false);

    engine.dispose();
  });

  it("lets only the newest overlapping seek resume playback", async () => {
    const master = new FakeVideo({ currentTime: 1, seekDelayMs: 20 });
    const slave = new FakeVideo({ currentTime: 1, seekDelayMs: 20 });
    const engine = engineFor(master, [slave]);

    await engine.play();
    const first = engine.seek(10);
    await sleep(2);
    const second = engine.seek(30);
    await Promise.all([first, second]);

    expect(master.currentTime).toBeCloseTo(30, 6);
    expect(slave.currentTime).toBeCloseTo(30, 6);
    expect(master.paused).toBe(false);
    expect(slave.paused).toBe(false);
    expect(useStore.getState().isPlaying).toBe(true);

    engine.dispose();
  });
});
