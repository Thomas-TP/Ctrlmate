import type { AllowlistedScript } from '../types/protocol';

export const MAX_SCRIPTS = 32;
const ID_RE = /^[a-z0-9_-]{1,32}$/;

export type ScriptIssue =
  | 'id_invalid'
  | 'id_duplicate'
  | 'name_empty'
  | 'exe_not_absolute'
  | 'dir_not_absolute'
  | 'too_many';

export function isAbsolutePath(p: string): boolean {
  const s = p.trim();
  return /^[A-Za-z]:[\\/]/.test(s) || s.startsWith('\\\\') || s.startsWith('/');
}

/** Mirrors the daemon validation (id, unique, absolute executable / working dir, <= 32 scripts). */
export function validateScripts(scripts: readonly AllowlistedScript[]): Map<number, ScriptIssue[]> {
  const issues = new Map<number, ScriptIssue[]>();
  const add = (i: number, issue: ScriptIssue) => issues.set(i, [...(issues.get(i) ?? []), issue]);
  const seen = new Map<string, number>();
  scripts.forEach((s, i) => {
    if (!ID_RE.test(s.id)) add(i, 'id_invalid');
    if (seen.has(s.id)) add(i, 'id_duplicate');
    seen.set(s.id, i);
    if (s.name.trim() === '') add(i, 'name_empty');
    if (!isAbsolutePath(s.executable)) add(i, 'exe_not_absolute');
    if (s.working_dir && s.working_dir.trim() !== '' && !isAbsolutePath(s.working_dir))
      add(i, 'dir_not_absolute');
  });
  if (scripts.length > MAX_SCRIPTS) add(MAX_SCRIPTS, 'too_many');
  return issues;
}

/** `Backup Photos` -> `backup_photos`, unique among `taken`. */
export function slugId(name: string, taken: ReadonlySet<string>): string {
  const base =
    name
      .toLowerCase()
      .normalize('NFD')
      .replace(/[̀-ͯ]/g, '')
      .replace(/[^a-z0-9]+/g, '_')
      .replace(/^_+|_+$/g, '')
      .slice(0, 28) || 'script';
  let id = base;
  for (let n = 2; taken.has(id); n++) id = `${base.slice(0, 28)}_${n}`;
  return id;
}

/** One argument per line (no shell: spaces inside an argument are preserved). */
export const argsToText = (args: readonly string[]): string => args.join('\n');
export const textToArgs = (text: string): string[] =>
  text.split(/\r?\n/).filter((l) => l.trim() !== '');
