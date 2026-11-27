import { useState } from 'react';
import { Play, Plus, Trash2 } from 'lucide-react';
import { cmd } from '../../../api/daemon';
import { useI18n } from '../../../i18n';
import {
  MAX_SCRIPTS,
  argsToText,
  slugId,
  textToArgs,
  validateScripts,
  type ScriptIssue,
} from '../../../lib/scripts';
import { useActions } from '../../../store/actions';
import type { AllowlistedScript, CommandResult, DaemonSnapshot } from '../../../types/protocol';
import { Banner, Button, EmptyNote, SwitchRow, TextField } from '../../../ui/kit';

interface Draft extends AllowlistedScript {
  argsText: string;
  isNew: boolean;
}

const toDraft = (s: AllowlistedScript, isNew = false): Draft => ({
  ...s,
  argsText: argsToText(s.args),
  isNew,
});
const fromDraft = ({ argsText, isNew: _isNew, ...s }: Draft): AllowlistedScript => ({
  ...s,
  args: textToArgs(argsText),
  working_dir: s.working_dir && s.working_dir.trim() !== '' ? s.working_dir.trim() : null,
  executable: s.executable.trim(),
  name: s.name.trim(),
});

export function ScriptsEditor({
  snap,
  onResult,
}: {
  snap: DaemonSnapshot;
  onResult: (label: string, r: CommandResult | null) => void;
}) {
  const { t } = useI18n();
  const actions = useActions();
  const [drafts, setDrafts] = useState<Draft[]>(() => snap.scripts.map((s) => toDraft(s)));
  const [dirty, setDirty] = useState(false);
  const [saving, setSaving] = useState(false);
  const [synced, setSynced] = useState(snap.scripts);
  if (!dirty && synced !== snap.scripts) {
    setSynced(snap.scripts);
    setDrafts(snap.scripts.map((s) => toDraft(s)));
  }

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
  const issueText = (list: ScriptIssue[] | undefined, which: ScriptIssue[]) => {
    const hit = list?.find((x) => which.includes(x));
    return hit ? t(`scripts.issue.${hit}`) : undefined;
  };
  const save = async () => {
    setSaving(true);
    const out = await actions.settings(
      { scripts: drafts.map(fromDraft) },
      { successKey: 'scripts.saved' },
    );
    setSaving(false);
    if (out) setDirty(false);
  };
  const run = async (s: AllowlistedScript) => {
    const r = await actions.run(cmd.press(`script_${s.id}`), { silent: true });
    onResult(s.name, r);
  };
  const canRun = snap.permissions.custom_scripts;

  return (
    <div className="space-y-4">
      <Banner tone="warning" title={t('scripts.warning.title')}>
        {t('scripts.warning.body')}
      </Banner>
      {!canRun ? <p className="text-xs text-muted">{t('scripts.exec_disabled')}</p> : null}
      {drafts.length === 0 ? (
        <EmptyNote title={t('scripts.empty.title')}>{t('scripts.empty.body')}</EmptyNote>
      ) : (
        <ul className="space-y-3">
          {drafts.map((s, i) => {
            const li = issues.get(i);
            return (
              <li key={`${s.id}-${i}`} className="space-y-3 rounded-xl border border-line p-4">
                <div className="grid grid-cols-2 gap-3">
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
                    error={issueText(li, ['id_invalid', 'id_duplicate'])}
                    onChange={(id) => update(i, { id })}
                  />
                </div>
                <TextField
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
                    className="mb-1 block text-[13px] font-medium text-fg"
                  >
                    {t('scripts.args')}
                  </label>
                  <textarea
                    id={`args-${i}`}
                    value={s.argsText}
                    rows={2}
                    onChange={(e) => update(i, { argsText: e.target.value })}
                    className="w-full rounded-control border border-line-strong bg-surface px-3 py-2 font-mono text-[13px] text-fg"
                  />
                  <p className="mt-1 text-xs text-subtle">{t('scripts.args.hint')}</p>
                </div>
                <TextField
                  label={t('scripts.working_dir')}
                  value={s.working_dir ?? ''}
                  mono
                  error={issueText(li, ['dir_not_absolute'])}
                  onChange={(working_dir) => update(i, { working_dir })}
                />
                <div className="flex items-center justify-between">
                  <SwitchRow
                    label={t('scripts.enabled')}
                    checked={s.enabled}
                    onChange={(enabled) => update(i, { enabled })}
                  />
                </div>
                <div className="flex justify-end gap-2">
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
                    onClick={() => {
                      setDirty(true);
                      setDrafts((d) => d.filter((_, n) => n !== i));
                    }}
                  >
                    {t('common.remove')}
                  </Button>
                </div>
              </li>
            );
          })}
        </ul>
      )}
      <div className="flex gap-2">
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
          disabled={!dirty || issues.size > 0}
          loading={saving}
          onClick={() => void save()}
        >
          {t('common.save')}
        </Button>
      </div>
    </div>
  );
}
