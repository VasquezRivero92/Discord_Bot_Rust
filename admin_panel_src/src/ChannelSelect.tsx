import React, { useEffect, useMemo, useRef, useState } from 'react';
import { Check, ChevronDown, X } from 'lucide-react';

export interface DiscordChannel {
  id: string;
  name: string;
  kind: 'text' | 'voice';
  parentId?: string | null;
  parentName?: string | null;
}

interface ChannelSelectProps {
  channels: DiscordChannel[];
  /** Solo muestra canales de este tipo (texto o voz); sin valor muestra todos. */
  kind?: 'text' | 'voice';
  /** Permite elegir más de un canal. */
  multiple?: boolean;
  value: string[];
  onChange: (ids: string[]) => void;
  placeholder?: string;
  /** Clases del botón, para que combine con cada formulario. */
  triggerClassName?: string;
}

const prefix = (c: DiscordChannel) => (c.kind === 'voice' ? '🔊 ' : '# ');

/**
 * Desplegable de canales de Discord con búsqueda. Si Discord no devolvió canales
 * (bot sin token o sin permisos) cae a un campo de texto con los IDs separados por coma.
 */
export function ChannelSelect({
  channels,
  kind,
  multiple = false,
  value,
  onChange,
  placeholder = 'Selecciona un canal',
  triggerClassName = 'bg-[#0A0A0A] border-2 border-white/10 focus:border-[#5865F2]/50',
}: ChannelSelectProps) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState('');
  const rootRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent) => {
      if (!rootRef.current?.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener('mousedown', close);
    return () => document.removeEventListener('mousedown', close);
  }, [open]);

  const byId = useMemo(() => new Map(channels.map((c) => [c.id, c])), [channels]);
  const visible = useMemo(() => {
    const q = query.trim().toLowerCase();
    return channels.filter(
      (c) => (!kind || c.kind === kind) && (!q || c.name.toLowerCase().includes(q) || c.id.includes(q)),
    );
  }, [channels, kind, query]);

  const groups = useMemo(() => {
    const out = new Map<string, DiscordChannel[]>();
    for (const c of visible) {
      const key = c.parentName || 'Sin categoría';
      out.set(key, [...(out.get(key) || []), c]);
    }
    return [...out.entries()];
  }, [visible]);

  if (channels.length === 0) {
    return (
      <input
        type="text"
        value={value.join(', ')}
        placeholder={multiple ? 'IDs separados por coma' : 'ID del canal'}
        onChange={(e) =>
          onChange(
            e.target.value
              .split(',')
              .map((s) => s.trim())
              .filter(Boolean),
          )
        }
        className={`${triggerClassName} text-xs font-mono font-black text-white rounded-none px-4 py-3.5 outline-none transition-colors w-full`}
      />
    );
  }

  const pick = (id: string) => {
    if (!multiple) {
      onChange([id]);
      setOpen(false);
      return;
    }
    onChange(value.includes(id) ? value.filter((v) => v !== id) : [...value, id]);
  };

  const label = (id: string) => {
    const c = byId.get(id);
    return c ? `${prefix(c)}${c.name}` : `ID ${id} (no encontrado)`;
  };

  return (
    <div ref={rootRef} className="relative w-full">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        className={`${triggerClassName} w-full flex items-center justify-between gap-2 text-left text-xs font-mono font-black text-white rounded-none px-4 py-3 outline-none transition-colors min-h-[46px]`}
      >
        <span className="flex flex-wrap gap-1.5 min-w-0">
          {value.length === 0 && <span className="text-neutral-500 font-normal">{placeholder}</span>}
          {value.map((id) =>
            multiple ? (
              <span key={id} className="inline-flex items-center gap-1 bg-white/10 px-2 py-0.5">
                {label(id)}
                <X
                  className="w-3 h-3 cursor-pointer hover:text-red-400"
                  onClick={(e) => {
                    e.stopPropagation();
                    onChange(value.filter((v) => v !== id));
                  }}
                />
              </span>
            ) : (
              <span key={id} className="truncate">
                {label(id)}
              </span>
            ),
          )}
        </span>
        <ChevronDown className="w-4 h-4 shrink-0 text-neutral-400" />
      </button>

      {open && (
        <div className="absolute z-50 mt-1 w-full bg-[#0A0A0A] border-2 border-white/15 shadow-xl">
          <input
            autoFocus
            type="text"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Buscar canal..."
            className="w-full bg-[#111] border-b border-white/10 text-xs font-mono text-white px-3 py-2 outline-none"
          />
          <div className="max-h-64 overflow-y-auto">
            {!multiple && value.length > 0 && (
              <button
                type="button"
                onClick={() => {
                  onChange([]);
                  setOpen(false);
                }}
                className="w-full text-left px-3 py-2 text-xs font-mono text-neutral-400 hover:bg-white/5"
              >
                Quitar selección
              </button>
            )}
            {groups.length === 0 && (
              <div className="px-3 py-3 text-xs font-mono text-neutral-500">Sin resultados</div>
            )}
            {groups.map(([group, items]) => (
              <div key={group}>
                <div className="px-3 pt-2 pb-1 text-[10px] font-mono font-black uppercase tracking-widest text-neutral-500">
                  {group}
                </div>
                {items.map((c) => {
                  const selected = value.includes(c.id);
                  return (
                    <button
                      type="button"
                      key={c.id}
                      onClick={() => pick(c.id)}
                      className={`w-full flex items-center justify-between px-3 py-2 text-xs font-mono hover:bg-white/5 ${
                        selected ? 'text-white' : 'text-neutral-300'
                      }`}
                    >
                      <span className="truncate">
                        {prefix(c)}
                        {c.name}
                      </span>
                      {selected && <Check className="w-3.5 h-3.5 text-[#5865F2]" />}
                    </button>
                  );
                })}
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}

/** IDs de un campo: la lista nueva si existe, o el ID único anterior. */
export function channelIdsOf(cfg: Record<string, any>, key: string): string[] {
  const list = cfg?.[`${key}s`];
  if (Array.isArray(list)) return list.filter(Boolean);
  return cfg?.[key] ? [cfg[key]] : [];
}
