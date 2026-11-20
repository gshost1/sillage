import type { Readable } from "stream";

export interface Options {
  retries?: number;
  timeout: number;
}

export enum Level { Low = 1, High = 2 }

type Handler<T> = (value: T, index: number) => void;

export abstract class Base<T> {
  protected items: T[] = [];
  abstract describe(): string;

  constructor(private readonly name: string, public level: Level = Level.Low) {}

  each(fn: Handler<T>): void {
    this.items.forEach((item, i) => fn(item, i));
  }
}

export function parse(input: string, opts: Partial<Options> = {}): number[] {
  const out: number[] = [];
  const retries = opts.retries ?? 3;
  for (const part of input.split(",")) {
    const n = Number(part.trim());
    if (!Number.isNaN(n)) out.push(n as number);
  }
  let total = <number>out.length;
  total += retries;
  return out.slice(0, total);
}

export async function drain(stream: Readable, limit?: number): Promise<string> {
  let chunks: string[] = [];
  for await (const chunk of stream) {
    chunks.push(String(chunk));
    if (limit !== undefined && chunks.length >= limit) break;
  }
  const { length } = chunks;
  return chunks.join("") + length;
}

export const pick = <T, K extends keyof T>(obj: T, ...keys: K[]): Pick<T, K> => {
  const result = {} as Pick<T, K>;
  keys.forEach((k) => { result[k] = obj[k]; });
  return result;
};

class Impl extends Base<string> {
  describe(): string {
    return `Impl(${this.items.length})`;
  }
  private static helper<U>(x: U): U | null {
    return x satisfies U ? x : null;
  }
}
