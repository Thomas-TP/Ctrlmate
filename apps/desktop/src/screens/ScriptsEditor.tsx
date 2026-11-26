import { useEffect, useState } from 'react';
import { Play, Plus, Trash2 } from 'lucide-react';
import { cmd } from '../api/daemon';
import { Button } from '../components/ui/Button';
import { SwitchRow, TextField } from '../components/ui/Form';
import { Banner, Card, EmptyState } from '../components/ui/Layout';
import { useI18n } from '../i18n';
import {
  MAX_SCRIPTS,
  argsToText,
  slugId,
  textToArgs,
  validateScripts,
  type ScriptIssue,
} from '../lib/scripts';
import { useActions } from '../store/actions';
import type { AllowlistedScript, CommandResult, DaemonSnapshot } from '../types/protocol';

interface Draft extends AllowlistedScript {
  argsText: string;
  isNew: boolean;
}

const toDraft = (s: AllowlistedScript, isNew = false): Draft => ({
  ...s,
  argsText: argsToText(s.args),
  isNew,
});
const fromDraft = ({ argsText, isNew, ...s }: Draft): AllowlistedScript => ({
  ...s,
  args: textToArgs(argsText),
  working_dir: s.working_dir && s.working_dir.trim() !== '' ? s.working_dir.trim() : null,
  executable: s.executable.trim(),
  name: s.name.trim(),
});

export function ScriptsEditor({
  snapshot: snap,
  onResult,
}: {
  snapshot: DaemonSnapshot;
  onResult: (label: string, r: CommandResult | null) => void;
}) {
  const { t } = useI18n();
  const actions = useActions();
  const [drafts, setDrafts] = useState<Draft[]>(() => snap.scripts.map((s) => toDraft(s)));
  const [dirty, setDirty] = useState(false);
  const [saving, setSaving] = useState(false);

  // Follow the daemon unless the user is editing.
  useEffect(() => {
    if (!dirty) setDrafts(snap.scripts.map((s) => toDraft(s)));
  }, [snap.scripts, dirty]);

  const issues = validateScripts(drafts.map(fromDraft));
  const update = (i: number, patch: Partial<Draft>) => {
    setDirty(true);
    setDrafts((d) => d.map((x, n) => (n === i ? { ...x, ...patch } : x)));
  };
  const add = () => {
    setDirty(true);
    setDrafts((d) => [
      ...d,
      toDraft(
        {
          id: slugId('script', new Set(d.map((x) => x.id))),
          name: '',
          executable: '',
          args: [],
          working_dir: null,
          enabled: true,
        },
        true,
      ),
    ]);
  };
  const remove = (i: number) => {
    setDirty(true);
    setDrafts((d) => d.filter((_, n) => n !== i));
  };
  const issueText = (list: ScriptIssue[] | undefined, which: ScriptIssue[]) => {
    const hit = list?.find((x) => which.includes(x));
    return hit ? t(`scripts.issue.${hit}`) : undefined;
  };

  const save = async () => {
    setSaving(true);
    const snapOut = await actions.settings(
      { scripts: drafts.map(fromDraft) },
      { successKey: 'scripts.saved' },
    );
    setSaving(false);
    if (snapOut) setDirty(false);
  };

  const run = async (s: AllowlistedScript) => {
    const r = await actions.run(cmd.press(`script_${s.id}`), { silent: true });
    onResult(s.name, r);
  };

  const canRun = snap.permissions.custom_scripts;
  const hasIssues = issues.size > 0;

  return (
    <Card
      title={t('scripts.title')}
      description={t('scripts.description')}
      actions={
        <>
          <Button
            size="sm"
            icon={<Plus className="size-4" aria-hidden />}
            onClick={add}
            disabled={drafts.length >= MAX_SCRIPTS}
          >
            {t('scripts.add')}
          </Button>
          <Button
            size="sm"
            variant="primary"
            disabled={!dirty || hasIssues}
            loading={saving}
            onClick={() => void save()}
          >
            {t('common.save')}
          </Button>
        </>
      }
    >
      <Banner tone="warning" title={t('scripts.warning.title')}>
        {t('scripts.warning.body')}
      </Banner>
      {!canRun ? <p className="mt-3 text-xs text-muted">{t('scripts.exec_disabled')}</p> : null}

      {drafts.length === 0 ? (
        <EmptyState title={t('scripts.empty.title')}>{t('scripts.empty.body')}</EmptyState>
      ) : (
        <ul className="mt-4 space-y-4">
          {drafts.map((s, i) => {
            const li = issues.get(i);
            return (
              <li key={`${s.id}-${i}`} className="rounded-control border border-line p-4">
                <div className="grid gap-3 sm:grid-cols-2">
                  <TextField
                    label={t('scripts.name')}
                    value={s.name}
                    error={issueText(li, ['name_empty'])}
                    onChange={(name) =>
                      update(i, {
                        name,
                        ...(s.isNew
                          ? {
                              id: slugId(
                                name,
                                new Set(drafts.filter((_, n) => n !== i).map((x) => x.id)),
                              ),
                            }
                          : {}),
                      })
                    }
                  />
                  <TextField
                    label={t('scripts.id')}
                    value={s.id}
                    mono
                    disabled={!s.isNew}
                    hint={t('scripts.id.hint')}
                    error={issueText(li, ['id_invalid', 'id_duplicate'])}
                    onChange={(id) => update(i, { id })}
                  />
                  <TextField
                    className="sm:col-span-2"
                    label={t('scripts.executable')}
                    value={s.executable}
                    mono
                    placeholder={t('scripts.executable.placeholder')}
                    error={issueText(li, ['exe_not_absolute'])}
                    onChange={(executable) => update(i, { executable })}
                  />
                  <div>
                    <label
                      htmlFor={`args-${i}`}
                      className="mb-1 block text-xs font-medium text-muted"
                    >
                      {t('scripts.args')}
                    </label>
                    <textarea
                      id={`args-${i}`}
                      value={s.argsText}
                      rows={3}
                      onChange={(e) => update(i, { argsText: e.target.value })}
                      aria-describedby={`args-${i}-h`}
                      className="w-full rounded-control border border-line-strong bg-surface px-3 py-2 font-mono text-[13px] text-fg"
                    />
                    <p id={`args-${i}-h`} className="mt-1 text-xs text-subtle">
                      {t('scripts.args.hint')}
                    </p>
                  </div>
                  <TextField
                    label={t('scripts.working_dir')}
                    value={s.working_dir ?? ''}
                    mono
                    error={issueText(li, ['dir_not_absolute'])}
                    onChange={(working_dir) => update(i, { working_dir })}
                  />
                </div>
                <div className="mt-2 flex flex-wrap items-center justify-between gap-2">
                  <div className="min-w-[200px] flex-1">
                    <SwitchRow
                      label={t('scripts.enabled')}
                      checked={s.enabled}
                      onChange={(enabled) => update(i, { enabled })}
                    />
                  </div>
                  <div className="flex items-center gap-2">
                    <Button
                      size="sm"
                      icon={<Play className="size-4" aria-hidden />}
                      disabled={dirty || s.isNew || !s.enabled || !canRun}
                      title={dirty ? t('scripts.run.save_first') : undefined}
                      onClick={() => void run(fromDraft(s))}
                    >
                      {t('scripts.run')}
                    </Button>
                    <Button
                      size="sm"
                      variant="ghost"
                      icon={<Trash2 className="size-4" aria-hidden />}
                      onClick={() => remove(i)}
                    >
                      {t('common.remove')}
                    </Button>
                  </div>
                </div>
              </li>
            );
          })}
        </ul>
      )}
    </Card>
  );
}
