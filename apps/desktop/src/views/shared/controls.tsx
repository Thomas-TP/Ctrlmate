import { useState } from 'react';
import { Check, Loader2, Lock, MonitorOff, Moon } from 'lucide-react';
import { cmd } from '../../api/daemon';
import { useI18n } from '../../i18n';
import { hasEntity, lightZones } from '../../lib/snapshot';
import { useActions } from '../../store/actions';
import { COOLING_PRESETS, type CoolingPreset, type DaemonSnapshot } from '../../types/protocol';
import { Button, Segmented, Slider, Switch, cn } from '../../ui/kit';
import { ConfirmDialog } from '../../ui/overlay';
import { percentToBrightness, brightnessToPercent, kelvinGradient } from '../../lib/color';

/** Requested preset + the pending choice that lasts until the daemon confirms it. */
export function usePresetChoice(snap: DaemonSnapshot) {
  const actions = useActions();
  const [pending, setPending] = useState<CoolingPreset | null>(null);
  const requested = snap.cooling_policy?.requested_preset ?? snap.cooling_preset;
  if (pending && requested === pending) setPending(null);
  const choose = async (p: CoolingPreset) => {
    setPending(p);
    const r = await actions.run(cmd.setCoolingPreset(p));
    if (!r?.ok) setPending(null);
  };
  return { selected: pending ?? requested, pending, choose };
}

/** Cooling preset picker. The daemon decides: the busy state lasts until it confirms. */
export function PresetPicker({
  snap,
  size = 'md',
  full,
}: {
  snap: DaemonSnapshot;
  size?: 'sm' | 'md';
  full?: boolean;
}) {
  const { t } = useI18n();
  const { selected, pending, choose } = usePresetChoice(snap);
  return (
    <Segmented
      label={t('cooling.presets.title')}
      size={size}
      full={full}
      value={selected}
      onChange={(p) => void choose(p)}
      options={COOLING_PRESETS.map((p) => ({
        value: p,
        label: t(`preset.${p}`),
        title: t(`preset.${p}.desc`),
        busy: pending === p,
      }))}
    />
  );
}

const KELVIN_PRESETS = [
  { k: 2700, key: 'light.warm' },
  { k: 4000, key: 'light.neutral' },
  { k: 6000, key: 'light.cool' },
] as const;

/** Master light: power, brightness and three colour-temperature shortcuts. */
export function MasterLight({ snap, compact }: { snap: DaemonSnapshot; compact?: boolean }) {
  const { t, num } = useI18n();
  const actions = useActions();
  const master = lightZones(snap).find((z) => z.isMaster) ?? lightZones(snap)[0];
  if (!master?.state || !master.controllable) {
    return <p className="text-[13px] text-muted">{t('lighting.no_state')}</p>;
  }
  const s = master.state;
  const caps = master.descriptor.light_caps;
  const minK = caps?.min_color_temp_kelvin ?? 2000;
  const maxK = caps?.max_color_temp_kelvin ?? 6500;
  const run = (patch: Parameters<typeof cmd.light>[1]) =>
    void actions.run(cmd.light(master.id, patch));
  return (
    <div className="space-y-3">
      <div className="flex items-center justify-between">
        <span className="text-sm font-medium text-fg">{t('lighting.power')}</span>
        <Switch
          label={t('lighting.power')}
          checked={s.is_on}
          onChange={(on) => run({ is_on: on })}
        />
      </div>
      {!compact || s.is_on ? (
        <Slider
          label={t('lighting.intensity')}
          value={brightnessToPercent(s.brightness)}
          min={1}
          max={100}
          disabled={!s.is_on}
          format={(v) => `${num(v)}\u202f%`}
          onCommit={(v) => run({ is_on: true, brightness: percentToBrightness(v) })}
        />
      ) : null}
      {caps?.supports_color_temp !== false ? (
        <div className="flex gap-1.5" role="group" aria-label={t('lighting.kelvin')}>
          {KELVIN_PRESETS.map((p) => {
            const k = Math.min(maxK, Math.max(minK, p.k));
            const on = s.color_mode === 'color_temp' && Math.abs(s.color_temp_kelvin - k) < 150;
            return (
              <button
                key={p.k}
                type="button"
                aria-pressed={on}
                onClick={() => run({ is_on: true, color_temp_kelvin: k })}
                className={cn(
                  'flex-1 rounded-lg border px-2 py-1.5 text-[12.5px] font-medium transition-colors',
                  on
                    ? 'border-accent bg-accent-soft text-accent-text'
                    : 'border-line text-muted hover:bg-surface-2 hover:text-fg',
                )}
              >
                {t(p.key)}
              </button>
            );
          })}
        </div>
      ) : null}
    </div>
  );
}

export { kelvinGradient };

type Quick = 'power_lock' | 'display_off' | 'power_sleep';

/** Lock, screens off and sleep (sleep asks first). */
export function QuickActions({ snap }: { snap: DaemonSnapshot }) {
  const { t } = useI18n();
  const actions = useActions();
  const [confirm, setConfirm] = useState(false);
  const [busy, setBusy] = useState(false);
  const has = (id: Quick) => hasEntity(snap, id);
  const press = async (id: Quick) => {
    setBusy(true);
    await actions.run(cmd.press(id));
    setBusy(false);
    setConfirm(false);
  };
  if (!has('power_lock') && !has('display_off') && !has('power_sleep')) return null;
  return (
    <>
      <div className="grid grid-cols-3 gap-2">
        {has('power_lock') ? (
          <Button
            size="sm"
            className="h-auto! flex-col gap-1 px-1 py-2.5 text-[12px] leading-tight whitespace-normal!"
            icon={<Lock className="size-4" aria-hidden />}
            onClick={() => void press('power_lock')}
          >
            {t('power.power_lock')}
          </Button>
        ) : null}
        {has('display_off') ? (
          <Button
            size="sm"
            className="h-auto! flex-col gap-1 px-1 py-2.5 text-[12px] leading-tight whitespace-normal!"
            icon={<MonitorOff className="size-4" aria-hidden />}
            onClick={() => void press('display_off')}
          >
            {t('power.display_off')}
          </Button>
        ) : null}
        {has('power_sleep') ? (
          <Button
            size="sm"
            className="h-auto! flex-col gap-1 px-1 py-2.5 text-[12px] leading-tight whitespace-normal!"
            icon={<Moon className="size-4" aria-hidden />}
            onClick={() => setConfirm(true)}
          >
            {t('power.power_sleep')}
          </Button>
        ) : null}
      </div>
      <ConfirmDialog
        open={confirm}
        title={t('power.power_sleep.confirm.title')}
        description={t('power.power_sleep.confirm.body')}
        confirmLabel={t('power.power_sleep')}
        loading={busy}
        onCancel={() => setConfirm(false)}
        onConfirm={() => void press('power_sleep')}
      />
    </>
  );
}

/** Large selectable preset list (cooling sheet and onboarding). */
export function PresetCards({ snap }: { snap: DaemonSnapshot }) {
  const { t } = useI18n();
  const { selected, pending, choose } = usePresetChoice(snap);
  return (
    <div className="space-y-2" role="radiogroup" aria-label={t('cooling.presets.title')}>
      {COOLING_PRESETS.map((p) => {
        const on = p === selected;
        return (
          <button
            key={p}
            type="button"
            role="radio"
            aria-checked={on}
            onClick={() => void choose(p)}
            className={cn(
              'flex w-full items-start gap-3 rounded-xl border px-4 py-3 text-left transition-colors',
              on ? 'border-accent bg-accent-soft' : 'border-line bg-surface hover:bg-surface-2',
            )}
          >
            <span
              className={cn(
                'mt-0.5 flex size-5 shrink-0 items-center justify-center rounded-full border',
                on ? 'border-accent bg-accent text-accent-fg' : 'border-line-strong',
              )}
              aria-hidden
            >
              {pending === p ? (
                <Loader2 className="size-3 cm-spin" />
              ) : on ? (
                <Check className="size-3" />
              ) : null}
            </span>
            <span className="min-w-0">
              <span className="block text-sm font-medium text-fg">{t(`preset.${p}`)}</span>
              <span className="block text-[13px] text-muted">{t(`preset.${p}.desc`)}</span>
            </span>
          </button>
        );
      })}
    </div>
  );
}

/** Wrapping chips for narrow spaces (the tray panel). */
export function PresetChips({ snap }: { snap: DaemonSnapshot }) {
  const { t } = useI18n();
  const { selected, pending, choose } = usePresetChoice(snap);
  return (
    <div
      className="flex flex-wrap gap-1.5"
      role="radiogroup"
      aria-label={t('cooling.presets.title')}
    >
      {COOLING_PRESETS.map((p) => {
        const on = p === selected;
        return (
          <button
            key={p}
            type="button"
            role="radio"
            aria-checked={on}
            title={t(`preset.${p}.desc`)}
            onClick={() => void choose(p)}
            className={cn(
              'inline-flex items-center gap-1.5 rounded-full border px-3 py-1.5 text-[13px] font-medium transition-colors',
              on
                ? 'border-accent bg-accent-soft text-accent-text'
                : 'border-line text-muted hover:bg-surface-2 hover:text-fg',
            )}
          >
            {pending === p ? <Loader2 className="size-3 cm-spin" aria-hidden /> : null}
            {t(`preset.${p}`)}
          </button>
        );
      })}
    </div>
  );
}
