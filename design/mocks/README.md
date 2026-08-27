# WireView Pro II desktop visual gate

These files are disposable Slint mockups. They contain fixture data and no
daemon, Varlink, USB, persistence, or production component wiring.
The production screenshots live in [`docs/assets/screenshots`](../../docs/assets/screenshots/).

The target user is someone diagnosing power delivery on a Linux workstation.
The window's single job is to expose a bad cable, connector, or power event
without hiding the measurements that explain it.

## A. Copper bus

Palette: black `#000000`, panel `#090B0D`, rule `#25292D`, copper `#F47A2A`,
ice `#8DD7FF`, fault `#FF476F`, white `#F5F7F8`.

Type: Orbitron for the power readout, Noto Sans for controls, Noto Sans Mono
for measurements.

Signature: one copper trace crosses the monitoring area like a PCB power bus.

```text
+----------+----------------------------------------------+
| nav      | device status                         session|
|          +---------------+------------------------------+
| overview | total power   | two-minute power bus         |
| pins     | voltage       |                              |
| faults   | current       +------------------------------+
| history  | temperature   | six-pin comparison           |
| config   | fan           |                              |
+----------+---------------+------------------------------+
| active fault and exact next action                      |
+---------------------------------------------------------+
```

## B. Bench console

Palette: black `#000000`, raised black `#050607`, rule `#30343A`, paper
`#ECEFF1`, cool gray `#8C949E`, signal blue `#6CB6FF`, fault `#FF5C68`.

Type: Noto Sans for navigation, Noto Sans Mono for the measurement ledger.

Signature: the six conductors form a lab ledger with a shared limit ruler.

```text
+---------------------------------------------------------+
| identity | connection | session | actions               |
+---------------------------------------------------------+
| overview  pins  faults  history  configure  theme       |
+------------------------------------------+--------------+
| six-pin measurement ledger               | total power  |
| P1 ...                                   | temperatures |
| P2 ...                                   | fault detail |
| ...                                      |              |
+------------------------------------------+--------------+
| current trace | power trace | voltage trace              |
+---------------------------------------------------------+
```

## C. Conductor field

Palette: black `#000000`, graphite `#0B0C0E`, conductor `#D9DEE3`, muted
`#6F7780`, warm current `#FFB15C`, cool voltage `#7CCBFF`, fault `#FF3B5C`.

Type: Orbitron for lane identifiers and total power, Noto Sans Mono for data,
Noto Sans for actions.

Signature: six horizontal conductor lanes make imbalance visible as geometry.

```text
+-------------+-------------------------------+-----------+
| total power | P1 ==========================  | faults    |
| voltage     | P2 =======================     | active    |
| current     | P3 ============================| recorded  |
| thermal     | P4 ======================      |           |
| fan         | P5 =========================   | action    |
|             | P6 ========================    |           |
+-------------+-------------------------------+-----------+
```

## Render checks

```bash
slint-viewer --check design/mocks/copper-bus.slint
slint-viewer --check design/mocks/bench-console.slint
slint-viewer --check design/mocks/conductor-field.slint

slint-viewer design/mocks/copper-bus.slint \
  --load-data design/mocks/fixtures/fault.json \
  --screenshot design/mocks/rendered/copper-bus-fault.png
```
