# camdl

**Compartmental Model Description Language**

camdl is a domain-specific language for stochastic compartmental epidemic
models. Write the model structure once in `.camdl`, compile to a flat JSON
intermediate representation, and run simulation or inference with the Rust
backend.

```
compartments { S, I, R }

parameters {
  beta  : rate
  gamma : rate
}

transitions {
  infection : S --> I @ beta * S * I / (S + I + R)
  recovery  : I --> R @ gamma * I
}
```

## Quick start

```bash
# Compile a model
camdlc model.camdl > model.ir.json

# Simulate
camdl simulate model.ir.json --params params.toml

# Inference (MLE via IF2)
camdl fit if2 --model model.ir.json --data cases.tsv

# Inference (Bayesian via PGAS)
camdl fit pgas --model model.ir.json --data cases.tsv
```

## Documentation

- **[camdl by Example](guide/intro.md)** — learn the language through worked examples
- **[Language Specification](language/spec.md)** — complete reference for `.camdl` syntax
- **[Inference Guide](inference/guide.md)** — particle filtering, IF2, PGAS, diagnostics
- **[IR Specification](architecture/ir-spec.md)** — the JSON intermediate representation
