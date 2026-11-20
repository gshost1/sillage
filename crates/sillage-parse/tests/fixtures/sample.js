#!/usr/bin/env node
'use strict';
import { readFile } from 'fs/promises';

const DEFAULTS = { retries: 3, verbose: false };

function merge(target, ...sources) {
  for (const src of sources) {
    for (const key in src) {
      if (Object.prototype.hasOwnProperty.call(src, key)) {
        target[key] = src[key];
      }
    }
  }
  return target;
}

const square = (n) => n * n;
const add = function (a, b = 1) { return a + b; };

export class Client {
  #secret = 42;
  static count = 0;

  constructor({ url, retries = DEFAULTS.retries }, [first, , third] = []) {
    this.url = url;
    this.retries = retries;
    Client.count++;
    this.pair = [first, third];
  }

  async fetchJson(path) {
    let attempt = 0;
    while (attempt < this.retries) {
      try {
        const res = await fetch(`${this.url}/${path}?t=${Date.now()}`);
        if (!res.ok) throw new Error(`HTTP ${res.status}`);
        return await res.json();
      } catch (err) {
        console.error(err.message);
        attempt += 1;
      } finally {
        /* noop */
      }
    }
    return null;
  }

  *ids(limit) {
    for (let i = 0; i < limit; i++) yield i;
  }

  get secret() { return this.#secret; }
}

function hoisting(flag) {
  if (flag) {
    var later = 1;
  }
  const { a, b: renamed, ...others } = { a: 1, b: 2, c: 3 };
  const re = /ab+c/gi;
  const items = [1, 2, 3].map((x, idx) => x * idx).filter(Boolean);
  label: for (const v of items) { if (v > 2) break label; }
  return later + a + renamed + others.c + re.source.length + items.length + square(2) + add(1);
}

const handlers = {
  onClick: function (event) { event.preventDefault(); },
  onKey(e) { return e.key === 'Enter' ? merge({}, DEFAULTS) : undefined; },
};

module.exports = { merge, hoisting, handlers, readFile };
