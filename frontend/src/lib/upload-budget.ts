// Keep this policy aligned with backend/src/routers/upload_body.rs.
export const uploadPolicy = {
  graceMs: 15_000,
  windowMs: 10_000,
  minBytesPerSecond: 64 * 1024,
  maxMs: 15 * 60_000,
};

export class UploadBudget {
  readonly ceilingMs: number;
  private readonly buckets: { at: number; bytes: number }[] = [];
  private readonly resolutionMs: number;
  private readonly started: number;

  constructor(
    limit: number,
    private readonly policy = uploadPolicy,
    private readonly now = () => performance.now(),
  ) {
    this.started = now();
    this.ceilingMs = Math.min(policy.maxMs, policy.graceMs + Math.ceil(limit / policy.minBytesPerSecond) * 1000);
    this.resolutionMs = Math.min(1000, policy.windowMs / 10);
  }

  record(bytes: number) {
    const now = this.now();
    this.prune(now);
    const last = this.buckets.at(-1);
    if (last && now - last.at < this.resolutionMs) last.bytes += bytes;
    else this.buckets.push({ at: now, bytes });
  }

  expired() {
    const now = this.now();
    if (now - this.started >= this.ceilingMs) return true;
    this.prune(now);
    return now - this.started >= this.policy.graceMs + this.policy.windowMs &&
      this.buckets.reduce((sum, bucket) => sum + bucket.bytes, 0) < this.policy.minBytesPerSecond * this.policy.windowMs / 1000;
  }

  private prune(now: number) {
    while (this.buckets[0] && this.buckets[0].at + this.resolutionMs <= now - this.policy.windowMs) this.buckets.shift();
  }
}
