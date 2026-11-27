import { useState } from 'react';
import { RefreshCw } from 'lucide-react';
import { cmd, daemon } from '../../../api/daemon';
import { useI18n } from '../../../i18n';
import {
  brightnessToPercent,
  hexToRgb,
  kelvinGradient,
  percentToBrightness,
  rgbToHex,
} from '../../../lib/color';
import { lightZones, orphanRgbDevices, type LightZone } from '../../../lib/snapshot';
import { useActions } from '../../../store/actions';
import type { DaemonSnapshot } from '../../../types/protocol';
import { Badge, Banner, Button, EmptyNote, SectionTitle, Slider, Switch } from '../../../ui/kit';

function Zone({ zone }: { zone: LightZone }) {
  const { t, code, num } = useI18n();
  const actions = useActions();
  const s = zone.state;
  const caps = zone.descriptor.light_caps;
  const run = (patch: Parameters<typeof cmd.light>[1]) =>
    void actions.run(cmd.light(zone.id, patch));
  const minK = caps?.min_color_temp_kelvin ?? 2000;
  const maxK = caps?.max_color_temp_kelvin ?? 6500;

  return (
    <li className="rounded-xl border border-line p-4">
      <div className="flex items-center justify-between gap-3">
        <div className="min-w-0">
          <p className="truncate text-sm font-medium text-fg">
            {zone.isMaster ? t('lighting.master') : zone.name}
          </p>
          {zone.isMaster ? (
            <p className="text-[13px] text-muted">{t('lighting.master.description')}</p>
          ) : caps?.led_count ? (
            <p className="text-[13px] text-muted">{`${caps.led_count} ${t('lighting.leds')}`}</p>
          ) : null}
        </div>
        {zone.controllable && s ? (
          <Switch label={zone.name} checked={s.is_on} onChange={(on) => run({ is_on: on })} />
        ) : (
          <Badge tone="warning">{t('lighting.not_controllable')}</Badge>
        )}
      </div>
      {zone.controllable && s ? (
        <div className="mt-4 space-y-4">
          <Slider
            label={t('lighting.intensity')}
            value={brightnessToPercent(s.brightness)}
            min={1}
            max={100}
            disabled={!s.is_on}
            format={(v) => `${num(v)}\u202f%`}
            onCommit={(v) => run({ is_on: true, brightness: percentToBrightness(v) })}
          />
          {caps?.supports_color_temp !== false ? (
            <Slider
              label={t('lighting.kelvin')}
              value={s.color_temp_kelvin}
              min={minK}
              max={maxK}
              step={100}
              disabled={!s.is_on}
              track={kelvinGradient(minK, maxK)}
              format={(v) => `${num(v)}\u202fK`}
              onCommit={(v) => run({ is_on: true, color_temp_kelvin: v })}
            />
          ) : null}
          {caps?.supports_rgb !== false ? (
            <div className="flex items-center justify-between">
              <label htmlFor={`rgb-${zone.id}`} className="text-[13px] font-medium text-fg">
                {t('lighting.color')}
              </label>
              <input
                id={`rgb-${zone.id}`}
                type="color"
                className="cm-color"
                disabled={!s.is_on}
                value={rgbToHex(s.rgb)}
                onChange={(e) => {
                  const rgb = hexToRgb(e.target.value);
                  if (rgb) run({ is_on: true, rgb });
                }}
              />
            </div>
          ) : null}
        </div>
      ) : zone.reason ? (
        <p className="mt-2 text-[13px] text-muted">{code('rgb.reason', zone.reason)}</p>
      ) : null}
    </li>
  );
}

export function LightingSheet({ snap }: { snap: DaemonSnapshot }) {
  const { t, code } = useI18n();
  const actions = useActions();
  const [busy, setBusy] = useState(false);
  const zones = lightZones(snap);
  const orphans = orphanRgbDevices(snap);

  const rescan = async () => {
    setBusy(true);
    await actions.snapshotCall(() => daemon.rescanHardware(), {
      successKey: 'lighting.rescan.done',
    });
    setBusy(false);
  };

  return (
    <div>
      {zones.length === 0 ? (
        <EmptyNote title={t('lighting.empty.title')}>{t('lighting.empty.body')}</EmptyNote>
      ) : (
        <ul className="space-y-3">
          {zones.map((z) => (
            <Zone key={z.id} zone={z} />
          ))}
        </ul>
      )}

      {orphans.length > 0 ? (
        <>
          <SectionTitle>{t('lighting.unavailable.title')}</SectionTitle>
          <p className="mb-2 text-[13px] text-muted">{t('lighting.unavailable.description')}</p>
          <ul className="divide-y divide-line rounded-xl border border-line px-4 text-sm">
            {orphans.map((d) => (
              <li key={d.id} className="py-2.5">
                <p className="font-medium text-fg">{d.name}</p>
                <p className="text-[13px] text-muted">{code('rgb.reason', d.reason)}</p>
              </li>
            ))}
          </ul>
        </>
      ) : null}

      {snap.rgb_backends.length > 0 ? (
        <div className="mt-4">
          <Banner tone="neutral">
            {t('lighting.backends', { list: snap.rgb_backends.join(', ') })}
          </Banner>
        </div>
      ) : null}
      <div className="mt-5">
        <Button
          icon={<RefreshCw className="size-4" aria-hidden />}
          loading={busy}
          onClick={() => void rescan()}
        >
          {t('lighting.rescan')}
        </Button>
      </div>
    </div>
  );
}
