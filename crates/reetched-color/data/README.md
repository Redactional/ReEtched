# Vendored OCIO configs

The `.ocio` files here are the ACES "CG" configs from the
[OpenColorIO-Config-ACES](https://github.com/AcademySoftwareFoundation/OpenColorIO-Config-ACES)
project, as built into OpenColorIO 2.6.0. Each file is the exact output of

```python
PyOpenColorIO.Config.CreateFromBuiltinConfig(name).serialize()
```

for the config of the same name (`tests/gen_reference.py` asserts this when regenerating the
reference data).

| File | ACES | OCIO profile |
| --- | --- | --- |
| `cg-config-v2.2.0_aces-v1.3_ocio-v2.4.ocio` | 1.3 | 2.4 |
| `cg-config-v4.0.0_aces-v2.0_ocio-v2.5.ocio` | 2.0 | 2.5 |
| `cg-config-v5.0.0_aces-v2.1_ocio-v2.6.ocio` | 2.1 | 2.6 |

License: BSD-3-Clause, Copyright Contributors to the OpenColorIO Project.
