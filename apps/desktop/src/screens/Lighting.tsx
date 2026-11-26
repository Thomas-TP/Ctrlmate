import { useEffect, useId, useRef, useState } from 'react';
import { RefreshCw, Sun } from 'lucide-react';
import { Button } from '../components/ui/Button';
import { Slider, SwitchRow } from '../components/ui/Form';
import { Badge, Card, EmptyState, KeyValue, PageHeader } from '../components/ui/Layout';
import { cmd, daemon } from '../api/daemon';
import { useI18n } from '../i18n';
import {
  brightnessToPercent,
  hexToRgb,
  kelvinGradient,
  percentToBrightness,
  rgbToHex,
} from '../lib/color';
import { lightZones, orphanRgbDevices, type LightZone } from '../lib/snapshot';
import { useActions } from '../store/actions';
import {
  MAX_COLOR_TEMP_KELVIN,
  MIN_COLOR_TEMP_KELVIN,
  type DaemonSnapshot,
  type RgbColor,
} from '../types/protocol';

const TRANSITION_MS = 400;

function ColorField({
  label,
  value,
  disabled,
  onCommit,
}: {
  label: string;
  value: RgbColor;
  disabled?: boolean;
  onCommit: (c: RgbColor) => void;
}) {
  const id = useId();
  const [draft, setDraft] = useState<string | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const hold = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const serverHex = rgbToHex(value);
  if (draft !== null && draft === serverHex) setDraft(null);
  useEffect(
    () => () => {
      clearTimeout(timer.current);
      clearTimeout(hold.current);
    },
    [],
  );
  const shown = draft ?? serverHex;
  return (
    <div className="flex items-center gap-3">
      <input
        id={id}
        type="color"
        className="cm-color"
        value={shown}
        disabled={disabled}
        onChange={(e) => {
          const hex = e.target.value;
          setDraft(hex);
          clearTimeout(timer.current);
          timer.current = setTimeout(() => {
            const rgb = hexToRgb(hex);
            if (rgb) onCommit(rgb);
            clearTimeout(hold.current);
            hold.current = setTimeout(() => setDraft(null), 2000);
          }, 150);
        }}
      />
      <div>
        <label htmlFor={id} className="block text-sm font-medium text-fg">
          {label}
        </label>
        <span className="font-mono text-xs text-muted">{shown.toUpperCase()}</span>
      </div>
    </div>
  );
}

function ZoneControls({ zone }: { zone: LightZone }) {
  const { t, num } = useI18n();
  const actions = useActions();
  const st = zone.state;
  const caps = zone.descriptor.light_caps;
  const send = (patch: Parameters<typeof cmd.light>[1]) =>
    void actions.run(
      cmd.light(zone.id, {
        ...patch,
        ...(caps?.supports_transition === false ? {} : { transition_ms: TRANSITION_MS }),
      }),
    );
  const kMin = caps?.min_color_temp_kelvin ?? MIN_COLOR_TEMP_KELVIN;
  const kMax = caps?.max_color_temp_kelvin ?? MAX_COLOR_TEMP_KELVIN;
  if (!st) return null;
  return (
    <div className="divide-y divide-line">
      <SwitchRow
        label={t('lighting.power')}
        description={st.is_on ? t('common.on') : t('common.off')}
        checked={st.is_on}
        onChange={(is_on) => send({ is_on })}
        disabled={!zone.controllable}
      />
      <div className="py-3">
        <Slider
          label={t('lighting.intensity')}
          value={brightnessToPercent(st.brightness)}
          min={0}
          max={100}
          disabled={!zone.controllable || !st.is_on}
          format={(v) => `${num(v)}\u202f%`}
          onCommit={(p) => send({ brightness: percentToBrightness(p) })}
        />
      </div>
      {caps?.supports_color_temp !== false ? (
        <div className="py-3">
          <Slider
            label={t('lighting.kelvin')}
            value={st.color_temp_kelvin}
            min={kMin}
            max={kMax}
            step={50}
            track={kelvinGradient(kMin, kMax)}
            disabled={!zone.controllable || !st.is_on}
            format={(v) => `${num(v)}\u202fK`}
            onCommit={(k) => send({ color_temp_kelvin: k })}
          />
          <div className="mt-1 flex justify-between text-xs text-subtle">
            <span>{t('lighting.kelvin.warm')}</span>
            <span>{t('lighting.kelvin.cool')}</span>
          </div>
        </div>
      ) : null}
      {caps?.supports_rgb !== false ? (
        <div className="py-3">
          <ColorField
            label={t('lighting.color')}
            value={st.rgb}
            disabled={!zone.controllable || !st.is_on}
            onCommit={(rgb) => send({ rgb })}
          />
        </div>
      ) : null}
      <p className="pt-3 text-xs text-subtle">
        {t('lighting.mode', { mode: t(`lighting.mode.${st.color_mode}`) })}
      </p>
    </div>
  );
}

function ZoneCard({ zone }: { zone: LightZone }) {
  const { t, code } = useI18n();
  const caps = zone.descriptor.light_caps;
  return (
    <Card
      title={zone.isMaster ? `${zone.name} · ${t('lighting.master')}` : zone.name}
      description={zone.isMaster ? t('lighting.master.description') : undefined}
      actions={
        zone.controllable ? (
          <Badge tone={zone.state?.is_on ? 'success' : 'neutral'}>
            {zone.state?.is_on ? t('common.on') : t('common.off')}
          </Badge>
        ) : (
          <Badge tone="warning">{t('lighting.not_controllable')}</Badge>
        )
      }
    >
      {zone.state === null ? (
        <p className="text-sm text-muted">
          {zone.reason ? code('rgb.reason', zone.reason) : t('lighting.no_state')}
        </p>
      ) : (
        <ZoneControls zone={zone} />
      )}
      {!zone.controllable && zone.reason ? (
        <p className="mt-3 text-sm text-muted">{code('rgb.reason', zone.reason)}</p>
      ) : null}
      {zone.devices.length > 0 || caps ? (
        <dl className="mt-4 border-t border-line pt-2">
          {zone.devices.map((d) => (
            <KeyValue key={d.id} label={d.name}>
              <span className="text-xs font-normal text-muted">{d.transport}</span>
            </KeyValue>
          ))}
          {caps && caps.led_count > 0 ? (
            <KeyValue label={t('lighting.leds')}>{caps.led_count}</KeyValue>
          ) : null}
        </dl>
      ) : null}
    </Card>
  );
}

export function Lighting({ snapshot: snap }: { snapshot: DaemonSnapshot }) {
  const { t, code } = useI18n();
  const actions = useActions();
  const [scanning, setScanning] = useState(false);
  const zones = lightZones(snap);
  const orphans = orphanRgbDevices(snap);
  const master = zones.filter((z) => z.isMaster);
  const others = zones.filter((z) => !z.isMaster);

  const rescan = async () => {
    setScanning(true);
    await actions.snapshotCall(() => daemon.rescanHardware(), {
      successKey: 'lighting.rescan.done',
    });
    setScanning(false);
  };

  return (
    <>
      <PageHeader
        title={t('lighting.title')}
        description={t('lighting.description')}
        actions={
          <Button
            icon={<RefreshCw className="size-4" aria-hidden />}
            loading={scanning}
            onClick={() => void rescan()}
          >
            {t('lighting.rescan')}
          </Button>
        }
      />
      {zones.length === 0 && orphans.length === 0 ? (
        <Card>
          <EmptyState
            icon={<Sun className="size-6" aria-hidden />}
            title={t('lighting.empty.title')}
          >
            {t('lighting.empty.body')}
          </EmptyState>
        </Card>
      ) : (
        <div className="space-y-4">
          {master.map((z) => (
            <ZoneCard key={z.id} zone={z} />
          ))}
          {others.length > 0 ? (
            <div className="grid gap-4 xl:grid-cols-2">
              {others.map((z) => (
                <ZoneCard key={z.id} zone={z} />
              ))}
            </div>
          ) : null}
          {orphans.length > 0 ? (
            <Card
              title={t('lighting.unavailable.title')}
              description={t('lighting.unavailable.description')}
            >
              <ul className="divide-y divide-line">
                {orphans.map((d) => (
                  <li
                    key={d.id}
                    className="flex flex-wrap items-center justify-between gap-2 py-2.5"
                  >
                    <span className="text-sm font-medium text-fg">{d.name}</span>
                    <span className="text-sm text-muted">{code('rgb.reason', d.reason)}</span>
                  </li>
                ))}
              </ul>
            </Card>
          ) : null}
          {snap.rgb_backends.length > 0 ? (
            <p className="text-xs text-subtle">
              {t('lighting.backends', { list: snap.rgb_backends.join(', ') })}
            </p>
          ) : null}
        </div>
      )}
    </>
  );
}
