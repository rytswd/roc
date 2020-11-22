use crate::x86_64::X86_64Backend;
use crate::{Backend, Env, Relocation, INLINED_SYMBOLS};
use bumpalo::collections::Vec;
use object::write;
use object::write::{Object, StandardSection, Symbol, SymbolSection};
use object::{
    Architecture, BinaryFormat, Endianness, RelocationEncoding, RelocationKind, SectionKind,
    SymbolFlags, SymbolKind, SymbolScope,
};
use roc_collections::all::MutMap;
use roc_module::symbol;
use roc_mono::ir::Proc;
use roc_mono::layout::Layout;
use target_lexicon::Triple;

const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn build_module<'a>(
    env: &'a Env,
    target: &Triple,
    procedures: MutMap<(symbol::Symbol, Layout<'a>), Proc<'a>>,
) -> Result<Object, String> {
    match target.architecture {
        target_lexicon::Architecture::X86_64 => {
            let mut output =
                Object::new(BinaryFormat::Elf, Architecture::X86_64, Endianness::Little);
            let text = output.section_id(StandardSection::Text);
            let data_section = output.section_id(StandardSection::Data);
            let comment = output.add_section(vec![], b"comment".to_vec(), SectionKind::OtherString);
            output.append_section_data(
                comment,
                format!("\0roc dev backend version {} \0", VERSION).as_bytes(),
                1,
            );

            // Setup layout_ids for procedure calls.
            let mut layout_ids = roc_mono::layout::LayoutIds::default();
            let mut procs = Vec::with_capacity_in(procedures.len(), env.arena);
            for ((sym, layout), proc) in procedures {
                // This is temporary until we support passing args to functions.
                if INLINED_SYMBOLS.contains(&sym) {
                    continue;
                }

                let fn_name = layout_ids
                    .get(sym, &layout)
                    .to_symbol_string(sym, &env.interns);

                let proc_symbol = Symbol {
                    name: fn_name.as_bytes().to_vec(),
                    value: 0,
                    size: 0,
                    kind: SymbolKind::Text,
                    // TODO: Depending on whether we are building a static or dynamic lib, this should change.
                    // We should use Dynamic -> anyone, Linkage -> static link, Compilation -> this module only.
                    scope: if env.exposed_to_host.contains(&sym) {
                        SymbolScope::Dynamic
                    } else {
                        SymbolScope::Linkage
                    },
                    weak: false,
                    section: SymbolSection::Section(text),
                    flags: SymbolFlags::None,
                };
                let proc_id = output.add_symbol(proc_symbol);
                procs.push((fn_name, proc_id, proc));
            }

            // Build procedures.
            let mut backend: X86_64Backend = Backend::new(env, target)?;
            for (fn_name, proc_id, proc) in procs {
                let mut local_data_index = 0;
                let (proc_data, relocations) = backend.build_proc(proc)?;
                let proc_offset = output.add_symbol_data(proc_id, text, proc_data, 16);
                for reloc in relocations {
                    let elfreloc = match reloc {
                        Relocation::LocalData { offset, data } => {
                            let data_symbol = write::Symbol {
                                name: format!("{}.data{}", fn_name, local_data_index)
                                    .as_bytes()
                                    .to_vec(),
                                value: 0,
                                size: 0,
                                kind: SymbolKind::Data,
                                scope: SymbolScope::Compilation,
                                weak: false,
                                section: write::SymbolSection::Section(data_section),
                                flags: SymbolFlags::None,
                            };
                            local_data_index += 1;
                            let data_id = output.add_symbol(data_symbol);
                            output.add_symbol_data(data_id, data_section, data, 4);
                            write::Relocation {
                                offset: *offset + proc_offset,
                                size: 32,
                                kind: RelocationKind::Relative,
                                encoding: RelocationEncoding::Generic,
                                symbol: data_id,
                                addend: -4,
                            }
                        }
                        Relocation::LinkedData { offset, name } => {
                            if let Some(sym_id) = output.symbol_id(name.as_bytes()) {
                                write::Relocation {
                                    offset: *offset + proc_offset,
                                    size: 32,
                                    kind: RelocationKind::GotRelative,
                                    encoding: RelocationEncoding::Generic,
                                    symbol: sym_id,
                                    addend: -4,
                                }
                            } else {
                                return Err(format!("failed to find symbol for {:?}", name));
                            }
                        }
                        Relocation::LinkedFunction { offset, name } => {
                            if let Some(sym_id) = output.symbol_id(name.as_bytes()) {
                                write::Relocation {
                                    offset: *offset + proc_offset,
                                    size: 32,
                                    kind: RelocationKind::PltRelative,
                                    encoding: RelocationEncoding::Generic,
                                    symbol: sym_id,
                                    addend: -4,
                                }
                            } else {
                                return Err(format!("failed to find symbol for {:?}", name));
                            }
                        }
                    };
                    output
                        .add_relocation(text, elfreloc)
                        .map_err(|e| format!("{:?}", e))?;
                }
            }
            Ok(output)
        }
        x => Err(format! {
        "the architecture, {:?}, is not yet implemented for elf",
        x}),
    }
}
