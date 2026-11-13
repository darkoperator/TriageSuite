// The grid's row blocks: rows are fetched 200 at a time by offset, and at most 12 blocks are kept. The cache is
// ordered by when a block was last drawn (touched); the least recently drawn goes first.

export const BLOCK_ROWS = 200;
export const MAX_BLOCKS = 12;

export const blockOf = (rowIndex: number): number => Math.floor(rowIndex / BLOCK_ROWS);

/** The blocks holding rows `first..=last` (clamped at row 0), in order. */
export function blocksCovering(first: number, last: number): number[] {
  const from = blockOf(Math.max(0, first));
  const to = blockOf(Math.max(0, last));
  const out: number[] = [];
  for (let b = from; b <= to; b++) out.push(b);
  return out;
}

export class BlockCache<T> {
  // A Map keeps insertion order: the first key is the least recently drawn.
  private readonly map = new Map<number, T>();

  constructor(private readonly max: number = MAX_BLOCKS) {}

  get size(): number {
    return this.map.size;
  }
  has(block: number): boolean {
    return this.map.has(block);
  }
  keys(): IterableIterator<number> {
    return this.map.keys();
  }
  /** The block without counting it as drawn. */
  peek(block: number): T | undefined {
    return this.map.get(block);
  }
  /** Counts the block as drawn just now. */
  touch(block: number): void {
    const v = this.map.get(block);
    if (v === undefined) return;
    this.map.delete(block);
    this.map.set(block, v);
  }
  /**
   * Stores a block (as the most recently drawn) and drops the least recently drawn ones beyond the cap. A block in
   * `protect` (the ones on screen) is never dropped: the cap yields rather than blank a row being drawn.
   */
  set(block: number, value: T, protect: ReadonlySet<number>): void {
    this.map.delete(block);
    this.map.set(block, value);
    for (const key of [...this.map.keys()]) {
      if (this.map.size <= this.max) break;
      if (key !== block && !protect.has(key)) this.map.delete(key);
    }
  }
  clear(): void {
    this.map.clear();
  }
}
