use crate::cdsl::formats::InstructionFormat;
use crate::cdsl::instructions::AllInstructions;
use crate::error;
use cranelift_srcgen::{fmtln, Formatter, Language};
use std::rc::Rc;

/// Which ISLE target are we generating code for?
#[derive(Clone, Copy, PartialEq, Eq)]
enum IsleTarget {
    /// Generating code for instruction selection and lowering.
    Lower,
    /// Generating code for CLIF to CLIF optimizations.
    Opt,
}

fn gen_common_isle(
    formats: &[Rc<InstructionFormat>],
    instructions: &AllInstructions,
    fmt: &mut Formatter,
    isle_target: IsleTarget,
) {
    use std::collections::{BTreeMap, BTreeSet};
    use std::fmt::Write;

    use crate::cdsl::formats::FormatField;

    fmt.multi_line(
        r#"
;; GENERATED BY `gen_isle`. DO NOT EDIT!!!
;;
;; This ISLE file defines all the external type declarations for Cranelift's
;; data structures that ISLE will process, such as `InstructionData` and
;; `Opcode`.
        "#,
    );
    fmt.empty_line();

    // Collect and deduplicate the immediate types from the instruction fields.
    let rust_name = |f: &FormatField| f.kind.rust_type.rsplit("::").next().unwrap();
    let fields = |f: &FormatField| f.kind.fields.clone();
    let immediate_types: BTreeMap<_, _> = formats
        .iter()
        .flat_map(|f| {
            f.imm_fields
                .iter()
                .map(|i| (rust_name(i), fields(i)))
                .collect::<Vec<_>>()
        })
        .collect();

    // Separate the `enum` immediates (e.g., `FloatCC`) from other kinds of
    // immediates.
    let (enums, others): (BTreeMap<_, _>, BTreeMap<_, _>) = immediate_types
        .iter()
        .partition(|(_, field)| field.enum_values().is_some());

    // Generate all the extern type declarations we need for the non-`enum`
    // immediates.
    fmt.line(";;;; Extern type declarations for immediates ;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;");
    fmt.empty_line();
    for ty in others.keys() {
        fmtln!(fmt, "(type {} (primitive {}))", ty, ty);
    }
    fmt.empty_line();

    // Generate the `enum` immediates, expanding all of the available variants
    // into ISLE.
    for (name, field) in enums {
        let field = field.enum_values().expect("only enums considered here");
        let variants = field.values().cloned().collect();
        gen_isle_enum(name, variants, fmt)
    }

    // Generate all of the value arrays we need for `InstructionData` as well as
    // the constructors and extractors for them.
    fmt.line(";;;; Value Arrays ;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;");
    fmt.empty_line();
    let value_array_arities: BTreeSet<_> = formats
        .iter()
        .filter(|f| f.typevar_operand.is_some() && !f.has_value_list && f.num_value_operands != 1)
        .map(|f| f.num_value_operands)
        .collect();
    for n in value_array_arities {
        fmtln!(fmt, ";; ISLE representation of `[Value; {}]`.", n);
        fmtln!(fmt, "(type ValueArray{} extern (enum))", n);
        fmt.empty_line();

        fmtln!(
            fmt,
            "(decl value_array_{} ({}) ValueArray{})",
            n,
            (0..n).map(|_| "Value").collect::<Vec<_>>().join(" "),
            n
        );
        fmtln!(
            fmt,
            "(extern constructor value_array_{} pack_value_array_{})",
            n,
            n
        );
        fmtln!(
            fmt,
            "(extern extractor infallible value_array_{} unpack_value_array_{})",
            n,
            n
        );
        fmt.empty_line();
    }

    // Generate all of the block arrays we need for `InstructionData` as well as
    // the constructors and extractors for them.
    fmt.line(";;;; Block Arrays ;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;");
    fmt.empty_line();
    let block_array_arities: BTreeSet<_> = formats
        .iter()
        .filter(|f| f.num_block_operands > 1)
        .map(|f| f.num_block_operands)
        .collect();
    for n in block_array_arities {
        fmtln!(fmt, ";; ISLE representation of `[BlockCall; {}]`.", n);
        fmtln!(fmt, "(type BlockArray{} extern (enum))", n);
        fmt.empty_line();

        fmtln!(
            fmt,
            "(decl block_array_{0} ({1}) BlockArray{0})",
            n,
            (0..n).map(|_| "BlockCall").collect::<Vec<_>>().join(" ")
        );

        fmtln!(
            fmt,
            "(extern constructor block_array_{0} pack_block_array_{0})",
            n
        );

        fmtln!(
            fmt,
            "(extern extractor infallible block_array_{0} unpack_block_array_{0})",
            n
        );
        fmt.empty_line();
    }

    // Generate the extern type declaration for `Opcode`.
    fmt.line(";;;; `Opcode` ;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;");
    fmt.empty_line();
    fmt.line("(type Opcode extern");
    fmt.indent(|fmt| {
        fmt.line("(enum");
        fmt.indent(|fmt| {
            for inst in instructions {
                fmtln!(fmt, "{}", inst.camel_name);
            }
        });
        fmt.line(")");
    });
    fmt.line(")");
    fmt.empty_line();

    // Generate the extern type declaration for `InstructionData`.
    fmtln!(
        fmt,
        ";;;; `InstructionData` ;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;",
    );
    fmt.empty_line();
    fmtln!(fmt, "(type InstructionData extern");
    fmt.indent(|fmt| {
        fmt.line("(enum");
        fmt.indent(|fmt| {
            for format in formats {
                let mut s = format!("({} (opcode Opcode)", format.name);
                if format.has_value_list {
                    s.push_str(" (args ValueList)");
                } else if format.num_value_operands == 1 {
                    s.push_str(" (arg Value)");
                } else if format.num_value_operands > 1 {
                    write!(&mut s, " (args ValueArray{})", format.num_value_operands).unwrap();
                }

                match format.num_block_operands {
                    0 => (),
                    1 => write!(&mut s, " (destination BlockCall)").unwrap(),
                    n => write!(&mut s, " (blocks BlockArray{n})").unwrap(),
                }

                for field in &format.imm_fields {
                    write!(
                        &mut s,
                        " ({} {})",
                        field.member,
                        field.kind.rust_type.rsplit("::").next().unwrap()
                    )
                    .unwrap();
                }
                s.push(')');
                fmt.line(&s);
            }
        });
        fmt.line(")");
    });
    fmt.line(")");
    fmt.empty_line();

    // Generate the helper extractors for each opcode's full instruction.
    fmtln!(
        fmt,
        ";;;; Extracting Opcode, Operands, and Immediates from `InstructionData` ;;;;;;;;",
    );
    fmt.empty_line();
    let ret_ty = match isle_target {
        IsleTarget::Lower => "Inst",
        IsleTarget::Opt => "Value",
    };
    for inst in instructions {
        if isle_target == IsleTarget::Opt
            && (inst.format.has_value_list || inst.value_results.len() != 1)
        {
            continue;
        }

        fmtln!(
            fmt,
            "(decl {} ({}{}) {})",
            inst.name,
            match isle_target {
                IsleTarget::Lower => "",
                IsleTarget::Opt => "Type ",
            },
            inst.operands_in
                .iter()
                .map(|o| {
                    let ty = o.kind.rust_type;
                    if ty == "&[Value]" {
                        "ValueSlice"
                    } else {
                        ty.rsplit("::").next().unwrap()
                    }
                })
                .collect::<Vec<_>>()
                .join(" "),
            ret_ty
        );
        fmtln!(fmt, "(extractor");
        fmt.indent(|fmt| {
            fmtln!(
                fmt,
                "({} {}{})",
                inst.name,
                match isle_target {
                    IsleTarget::Lower => "",
                    IsleTarget::Opt => "ty ",
                },
                inst.operands_in
                    .iter()
                    .map(|o| { o.name })
                    .collect::<Vec<_>>()
                    .join(" ")
            );

            let mut s = format!(
                "(inst_data{} (InstructionData.{} (Opcode.{})",
                match isle_target {
                    IsleTarget::Lower => "",
                    IsleTarget::Opt => " ty",
                },
                inst.format.name,
                inst.camel_name
            );

            // Value and varargs operands.
            if inst.format.has_value_list {
                // The instruction format uses a value list, but the
                // instruction itself might have not only a `&[Value]`
                // varargs operand, but also one or more `Value` operands as
                // well. If this is the case, then we need to read them off
                // the front of the `ValueList`.
                let values: Vec<_> = inst
                    .operands_in
                    .iter()
                    .filter(|o| o.is_value())
                    .map(|o| o.name)
                    .collect();
                let varargs = inst
                    .operands_in
                    .iter()
                    .find(|o| o.is_varargs())
                    .unwrap()
                    .name;
                if values.is_empty() {
                    write!(&mut s, " (value_list_slice {varargs})").unwrap();
                } else {
                    write!(
                        &mut s,
                        " (unwrap_head_value_list_{} {} {})",
                        values.len(),
                        values.join(" "),
                        varargs
                    )
                    .unwrap();
                }
            } else if inst.format.num_value_operands == 1 {
                write!(
                    &mut s,
                    " {}",
                    inst.operands_in.iter().find(|o| o.is_value()).unwrap().name
                )
                .unwrap();
            } else if inst.format.num_value_operands > 1 {
                let values = inst
                    .operands_in
                    .iter()
                    .filter(|o| o.is_value())
                    .map(|o| o.name)
                    .collect::<Vec<_>>();
                assert_eq!(values.len(), inst.format.num_value_operands);
                let values = values.join(" ");
                write!(
                    &mut s,
                    " (value_array_{} {})",
                    inst.format.num_value_operands, values,
                )
                .unwrap();
            }

            // Immediates.
            let imm_operands: Vec<_> = inst
                .operands_in
                .iter()
                .filter(|o| !o.is_value() && !o.is_varargs() && !o.kind.is_block())
                .collect();
            assert_eq!(imm_operands.len(), inst.format.imm_fields.len(),);
            for op in imm_operands {
                write!(&mut s, " {}", op.name).unwrap();
            }

            // Blocks.
            let block_operands: Vec<_> = inst
                .operands_in
                .iter()
                .filter(|o| o.kind.is_block())
                .collect();
            assert_eq!(block_operands.len(), inst.format.num_block_operands);
            assert!(block_operands.len() <= 2);

            if !block_operands.is_empty() {
                if block_operands.len() == 1 {
                    write!(&mut s, " {}", block_operands[0].name).unwrap();
                } else {
                    let blocks: Vec<_> = block_operands.iter().map(|o| o.name).collect();
                    let blocks = blocks.join(" ");
                    write!(
                        &mut s,
                        " (block_array_{} {})",
                        inst.format.num_block_operands, blocks,
                    )
                    .unwrap();
                }
            }

            s.push_str("))");
            fmt.line(&s);
        });
        fmt.line(")");

        // Generate a constructor if this is the mid-end prelude.
        if isle_target == IsleTarget::Opt {
            fmtln!(
                fmt,
                "(rule ({} ty {})",
                inst.name,
                inst.operands_in
                    .iter()
                    .map(|o| o.name)
                    .collect::<Vec<_>>()
                    .join(" ")
            );
            fmt.indent(|fmt| {
                let mut s = format!(
                    "(make_inst ty (InstructionData.{} (Opcode.{})",
                    inst.format.name, inst.camel_name
                );

                // Handle values. Note that we skip generating
                // constructors for any instructions with variadic
                // value lists. This is fine for the mid-end because
                // in practice only calls and branches (for branch
                // args) use this functionality, and neither can
                // really be optimized or rewritten in the mid-end
                // (currently).
                //
                // As a consequence, we only have to handle the
                // one-`Value` case, in which the `Value` is directly
                // in the `InstructionData`, and the multiple-`Value`
                // case, in which the `Value`s are in a
                // statically-sized array (e.g. `[Value; 2]` for a
                // binary op).
                assert!(!inst.format.has_value_list);
                if inst.format.num_value_operands == 1 {
                    write!(
                        &mut s,
                        " {}",
                        inst.operands_in.iter().find(|o| o.is_value()).unwrap().name
                    )
                    .unwrap();
                } else if inst.format.num_value_operands > 1 {
                    // As above, get all bindings together, and pass
                    // to a sub-term; here we use a constructor to
                    // build the value array.
                    let values = inst
                        .operands_in
                        .iter()
                        .filter(|o| o.is_value())
                        .map(|o| o.name)
                        .collect::<Vec<_>>();
                    assert_eq!(values.len(), inst.format.num_value_operands);
                    let values = values.join(" ");
                    write!(
                        &mut s,
                        " (value_array_{}_ctor {})",
                        inst.format.num_value_operands, values
                    )
                    .unwrap();
                }

                if inst.format.num_block_operands > 0 {
                    let blocks: Vec<_> = inst
                        .operands_in
                        .iter()
                        .filter(|o| o.kind.is_block())
                        .map(|o| o.name)
                        .collect();
                    if inst.format.num_block_operands == 1 {
                        write!(&mut s, " {}", blocks.first().unwrap(),).unwrap();
                    } else {
                        write!(
                            &mut s,
                            " (block_array_{} {})",
                            inst.format.num_block_operands,
                            blocks.join(" ")
                        )
                        .unwrap();
                    }
                }

                // Immediates (non-value args).
                for o in inst
                    .operands_in
                    .iter()
                    .filter(|o| !o.is_value() && !o.is_varargs() && !o.kind.is_block())
                {
                    write!(&mut s, " {}", o.name).unwrap();
                }
                s.push_str("))");
                fmt.line(&s);
            });
            fmt.line(")");
        }

        fmt.empty_line();
    }
}

fn gen_opt_isle(
    formats: &[Rc<InstructionFormat>],
    instructions: &AllInstructions,
    fmt: &mut Formatter,
) {
    gen_common_isle(formats, instructions, fmt, IsleTarget::Opt);
}

fn gen_lower_isle(
    formats: &[Rc<InstructionFormat>],
    instructions: &AllInstructions,
    fmt: &mut Formatter,
) {
    gen_common_isle(formats, instructions, fmt, IsleTarget::Lower);
}

/// Generate an `enum` immediate in ISLE.
fn gen_isle_enum(name: &str, mut variants: Vec<&str>, fmt: &mut Formatter) {
    variants.sort();
    let prefix = format!(";;;; Enumerated Immediate: {name} ");
    fmtln!(fmt, "{:;<80}", prefix);
    fmt.empty_line();
    fmtln!(fmt, "(type {} extern", name);
    fmt.indent(|fmt| {
        fmt.line("(enum");
        fmt.indent(|fmt| {
            for variant in variants {
                fmtln!(fmt, "{}", variant);
            }
        });
        fmt.line(")");
    });
    fmt.line(")");
    fmt.empty_line();
}

pub(crate) fn generate(
    formats: &[Rc<InstructionFormat>],
    all_inst: &AllInstructions,
    isle_opt_filename: &str,
    isle_lower_filename: &str,
    isle_dir: &std::path::Path,
) -> Result<(), error::Error> {
    // ISLE DSL: mid-end ("opt") generated bindings.
    let mut fmt = Formatter::new(Language::Isle);
    gen_opt_isle(&formats, all_inst, &mut fmt);
    fmt.write(isle_opt_filename, isle_dir)?;

    // ISLE DSL: lowering generated bindings.
    let mut fmt = Formatter::new(Language::Isle);
    gen_lower_isle(&formats, all_inst, &mut fmt);
    fmt.write(isle_lower_filename, isle_dir)?;

    Ok(())
}
