use crate::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

#[async_trait::async_trait]
pub trait IPackageValidateService {
    /// 验证打包内容的格式，返回正确或错误原因
    async fn package_validate(&self, validate_data: ValidateData)
        -> anyhow::Result<CommandPreview>;
}

/// 包验证服务
pub struct PackageValidateService {}

// /// 清单文件名称
// const MANIFEST_FILE_NAME: &'static str = "manifest";
// /// yaml 格式文件后缀名 1
// const YML_EXTENTION1: &'static str = "yml";
// /// yaml 格式文件后缀名 2
// const YML_EXTENTION2: &'static str = "yaml";

/// 验证所需数据结构
#[derive(Debug, Serialize, Deserialize)]
pub struct ValidateData {
    /// 软件包数据
    pub software_data: SoftwareData,
    /// 用例包数据
    pub usecase_data: UsecaseData,
}

#[async_trait::async_trait]
impl IPackageValidateService for PackageValidateService {
    async fn package_validate(
        &self,
        validate_data: ValidateData,
    ) -> anyhow::Result<CommandPreview> {
        let software_data = validate_data.software_data;
        let usecase_data = validate_data.usecase_data;

        let usecase_spec = usecase_data.spec;
        let argument_materials = software_data.arguments;
        let environment_materials = software_data.environments;
        let filesome_input_materials = software_data.filesome_inputs;
        let filesome_output_materials = software_data.filesome_outputs;
        let software_spec = software_data.spec;
        let template_file_infos = usecase_data.template_file_infos;
        let collected_outs = usecase_data.collected_outs;

        let input_slots = usecase_spec.input_slots;

        let mut argument_formats_sorts = HashMap::<usize, FormatFillPreview>::new();
        let mut environment_formats = HashMap::<String, FormatFillPreview>::new();
        let mut file_infos = vec![];
        let mut std_in = InDescriptor::default();
        // 模板描述符及其键填充的输入插槽的对应关系集合
        let mut templates_kv_map = HashMap::<String, HashMap<String, InDescriptor>>::new();
        let mut collect_previews = vec![];

        for input_slot in input_slots.iter() {
            let input_slot_descriptor = input_slot.descriptor().to_owned();

            match input_slot {
                InputSlot::Text { ref_materials, .. } => {
                    // 处理文本输入的所有挂载
                    for ref_material in ref_materials.iter() {
                        // 判断挂载类型
                        match ref_material {
                            TextRef::ArgRef {
                                descriptor,
                                sort,
                                placeholder_nth,
                            } => {
                                // 获取参数格式
                                let argument_format =
                                    Self::argument_format(&argument_materials, descriptor)?;

                                argument_formats_sorts
                                    .entry(*sort)
                                    .or_insert(argument_format)
                                    .placeholder_fill_map
                                    .insert(
                                        *placeholder_nth,
                                        InDescriptor::InputSlot {
                                            descriptor: input_slot_descriptor.to_owned(),
                                        },
                                    );
                            }

                            TextRef::EnvRef {
                                descriptor,
                                placeholder_nth,
                            } => {
                                let (key, value_format) = Self::environment_kv_format(
                                    &environment_materials,
                                    descriptor,
                                )?;

                                environment_formats
                                    .entry(key)
                                    .or_insert(value_format)
                                    .placeholder_fill_map
                                    .insert(
                                        *placeholder_nth,
                                        InDescriptor::InputSlot {
                                            descriptor: input_slot_descriptor.to_owned(),
                                        },
                                    );
                            }

                            TextRef::StdIn => {
                                std_in = InDescriptor::InputSlot {
                                    descriptor: input_slot_descriptor.to_owned(),
                                };
                            }

                            TextRef::TemplateRef {
                                descriptor,
                                ref_keys,
                            } => {
                                for ref_key in ref_keys.iter() {
                                    templates_kv_map
                                        .entry(descriptor.to_owned())
                                        .or_insert(HashMap::new())
                                        .insert(
                                            ref_key.to_owned(),
                                            InDescriptor::InputSlot {
                                                descriptor: input_slot_descriptor.to_owned(),
                                            },
                                        );
                                }
                            }
                        }
                    }
                }

                InputSlot::File { ref_materials, .. } => {
                    for ref_material in ref_materials.iter() {
                        match ref_material {
                            FileRef::ArgRef {
                                descriptor,
                                placeholder_nth,
                                sort,
                            } => {
                                let argument_format =
                                    Self::argument_format(&argument_materials, descriptor)?;

                                argument_formats_sorts
                                    .entry(*sort)
                                    .or_insert(argument_format)
                                    .placeholder_fill_map
                                    .insert(
                                        *placeholder_nth,
                                        InDescriptor::InputSlot {
                                            descriptor: input_slot_descriptor.to_owned(),
                                        },
                                    );
                            }
                            FileRef::EnvRef {
                                descriptor,
                                placeholder_nth,
                            } => {
                                let (key, value_format) = Self::environment_kv_format(
                                    &environment_materials,
                                    descriptor,
                                )?;

                                environment_formats
                                    .entry(key)
                                    .or_insert(value_format)
                                    .placeholder_fill_map
                                    .insert(
                                        *placeholder_nth,
                                        InDescriptor::InputSlot {
                                            descriptor: input_slot_descriptor.to_owned(),
                                        },
                                    );
                            }

                            FileRef::StdIn => {
                                std_in = InDescriptor::InputSlot {
                                    descriptor: input_slot_descriptor.to_owned(),
                                };
                            }

                            FileRef::FileInputRef(file_material_descriptor) => {
                                let file_material = filesome_input_materials
                                    .iter()
                                    .find(|el| el.descriptor.eq(file_material_descriptor))
                                    .ok_or(anyhow::anyhow!(
                                        "No such file material: {file_material_descriptor}"
                                    ))?;
                                file_infos.push(FileInfoPreview::ConstInput {
                                    from: InDescriptor::InputSlot {
                                        descriptor: input_slot_descriptor.to_owned(),
                                    },
                                    input_material_descriptor: file_material_descriptor.to_owned(),
                                    path: match file_material.file_kind.to_owned() {
                                        FileKind::Normal(path) => path,
                                        FileKind::Batched(path) => path,
                                    },
                                })
                            }

                            FileRef::TemplateRef {
                                descriptor,
                                ref_keys,
                            } => {
                                for ref_key in ref_keys.iter() {
                                    templates_kv_map
                                        .entry(descriptor.to_owned())
                                        .or_insert(HashMap::new())
                                        .insert(
                                            ref_key.to_owned(),
                                            InDescriptor::InputSlot {
                                                descriptor: input_slot_descriptor.to_owned(),
                                            },
                                        );
                                }
                            }
                        }
                    }
                }
            }
        }

        for template_file in usecase_spec.template_files.iter() {
            for as_file_name in template_file.as_file_name.iter() {
                match as_file_name {
                    FileRef::ArgRef { descriptor, .. } => {
                        argument_materials.iter().find(|el| el.descriptor.eq(descriptor)).ok_or(
                            anyhow::anyhow!("No such argument material descriptor: {descriptor}"),
                        )?;
                    }
                    FileRef::EnvRef { descriptor, .. } => {
                        environment_materials
                            .iter()
                            .find(|el| el.descriptor.eq(descriptor))
                            .ok_or(anyhow::anyhow!(
                                "No such environment material descriptor: {descriptor}"
                            ))?;
                    }
                    FileRef::StdIn => {}
                    FileRef::FileInputRef(descriptor) => {
                        filesome_input_materials
                            .iter()
                            .find(|el| el.descriptor.eq(descriptor))
                            .ok_or(anyhow::anyhow!(
                                "No such filesome input material descriptor: {descriptor}"
                            ))?;
                    }
                    FileRef::TemplateRef { .. } => {
                        anyhow::bail!("unimplemented")
                    }
                }
            }
        }
        // 遍历使用的 template
        for (template_descriptor, _) in templates_kv_map.iter() {
            let using_template_file = usecase_spec
                .template_files
                .iter()
                .find(|el| el.descriptor.eq(template_descriptor))
                .ok_or(anyhow::anyhow!(
                    "No such template with descriptor: {template_descriptor}"
                ))?;
            template_file_infos
                .iter()
                .find(|el| el.descriptor.eq(template_descriptor))
                .ok_or(anyhow::anyhow!(
                    "No such template with descriptor: {template_descriptor}"
                ))?;

            for as_content in using_template_file.as_content.iter() {
                match as_content {
                    TextRef::ArgRef {
                        descriptor,
                        placeholder_nth,
                        sort,
                    } => {
                        let argument_format =
                            Self::argument_format(&argument_materials, descriptor)?;

                        argument_formats_sorts
                            .entry(*sort)
                            .or_insert(argument_format)
                            .placeholder_fill_map
                            .insert(
                                *placeholder_nth,
                                InDescriptor::Template {
                                    descriptor: template_descriptor.to_owned(),
                                },
                            );
                    }

                    TextRef::EnvRef {
                        descriptor,
                        placeholder_nth,
                    } => {
                        let (key, value_format) =
                            Self::environment_kv_format(&environment_materials, descriptor)?;

                        environment_formats
                            .entry(key)
                            .or_insert(value_format)
                            .placeholder_fill_map
                            .insert(
                                *placeholder_nth,
                                InDescriptor::Template {
                                    descriptor: template_descriptor.to_owned(),
                                },
                            );
                    }

                    TextRef::StdIn => {
                        std_in = InDescriptor::Template {
                            descriptor: template_descriptor.to_owned(),
                        };
                    }

                    TextRef::TemplateRef {
                        descriptor: _,
                        ref_keys: _,
                    } => anyhow::bail!(
                        "TemplateRef To Text and File for template is not implemented!"
                    ),
                }
            }

            if !(using_template_file.as_file_name.is_empty()
                || using_template_file.as_file_name.len() == 1
                    && matches!(
                        using_template_file.as_file_name.get(0).unwrap(),
                        FileRef::FileInputRef(_)
                    ))
            {
                file_infos.push(FileInfoPreview::DynamicInput {
                    from: InDescriptor::Template {
                        descriptor: template_descriptor.to_owned(),
                    },
                });
            }

            for as_file_name in using_template_file.as_file_name.iter() {
                match as_file_name {
                    FileRef::ArgRef {
                        descriptor,
                        placeholder_nth,
                        sort,
                    } => {
                        let argument_format =
                            Self::argument_format(&argument_materials, descriptor)?;

                        argument_formats_sorts
                            .entry(*sort)
                            .or_insert(argument_format)
                            .placeholder_fill_map
                            .insert(
                                *placeholder_nth,
                                InDescriptor::Template {
                                    descriptor: template_descriptor.to_owned(),
                                },
                            );
                    }

                    FileRef::EnvRef {
                        descriptor,
                        placeholder_nth,
                    } => {
                        let (key, value_format) =
                            Self::environment_kv_format(&environment_materials, descriptor)?;

                        environment_formats
                            .entry(key)
                            .or_insert(value_format)
                            .placeholder_fill_map
                            .insert(
                                *placeholder_nth,
                                InDescriptor::Template {
                                    descriptor: template_descriptor.to_owned(),
                                },
                            );
                    }

                    FileRef::StdIn => {
                        std_in = InDescriptor::Template {
                            descriptor: template_descriptor.to_owned(),
                        };
                    }

                    FileRef::FileInputRef(input_material_descriptor) => {
                        let input_material=filesome_input_materials
                        .iter()
                        .find(|el| el.descriptor.eq(input_material_descriptor))
                        .ok_or(anyhow::anyhow!("No such filesome input material with descriptor:{input_material_descriptor}"))?;
                        match input_material.file_kind.to_owned() {
                            FileKind::Normal(_) => file_infos.push(FileInfoPreview::ConstInput {
                                from: InDescriptor::Template {
                                    descriptor: template_descriptor.to_owned(),
                                },
                                input_material_descriptor: input_material_descriptor.to_owned(),
                                path: match input_material.file_kind.to_owned() {
                                    FileKind::Normal(path) => path,
                                    FileKind::Batched(path) => path,
                                },
                            }),
                            FileKind::Batched(_) => file_infos.push(FileInfoPreview::ConstInput {
                                from: InDescriptor::Template {
                                    descriptor: template_descriptor.to_owned(),
                                },
                                input_material_descriptor: input_material_descriptor.to_owned(),
                                path: match input_material.file_kind.to_owned() {
                                    FileKind::Normal(path) => path,
                                    FileKind::Batched(path) => path,
                                },
                            }),
                        }
                    }

                    FileRef::TemplateRef {
                        descriptor: _,
                        ref_keys: _,
                    } => anyhow::bail!(
                        "TemplateRef To Text and File for template is not implemented!"
                    ),
                }
            }
        }

        for output_slot in usecase_spec.output_slots.iter() {
            match output_slot {
                OutputSlot::Text {
                    collected_out_descriptor,
                    optional,
                    ..
                } => {
                    let collected_out = collected_outs
                        .iter()
                        .find(|el| el.descriptor.eq(collected_out_descriptor))
                        .ok_or(anyhow::anyhow!(
                            "No such collect out with descriptor: {collected_out_descriptor}"
                        ))?;
                    // 解析从哪收集
                    let from = match collected_out.from.to_owned() {
                        CollectFrom::FileOut(fileout_descriptor) => {
                            let mut filesome_out_material = Option::<FilesomeOutput>::None;
                            for out_slot in usecase_spec.output_slots.iter() {
                                if let OutputSlot::File {
                                    descriptor, origin, ..
                                } = out_slot
                                {
                                    if descriptor.eq(&fileout_descriptor) {
                                        match origin {
                                            FileOutOrigin::CollectedOut(_) => anyhow::bail!(
                                                "Collect CollectedOut file or text is not implemented!"
                                            ),
                                            FileOutOrigin::UsecaseOut(
                                                file_out_and_appointed_by,
                                            ) => {
                                                filesome_out_material= Some(filesome_output_materials
                                                    .iter()
                                                    .find(|el| {
                                                        el.descriptor.eq(&file_out_and_appointed_by
                                                            .file_out_material_descriptor)
                                                    })
                                                    .ok_or(anyhow::anyhow!("No such filesome output material with descriptor: {}",file_out_and_appointed_by.file_out_material_descriptor))?.to_owned());
                                                    if let AppointedBy::InputSlot { ref text_input_descriptor }= file_out_and_appointed_by.kind{
                                                     let mut input_slot_descriptor = None;
                                                     for input_slot in input_slots.iter(){
                                                        match input_slot{
                                                            InputSlot::Text { descriptor, ..}=>{
                                                                if descriptor.eq(text_input_descriptor){
                                                                  input_slot_descriptor = Some(descriptor.to_owned());
                                                                  break;
                                                                }
                                                            },
                                                            InputSlot::File { descriptor, .. } =>{
                                                                if descriptor.eq(text_input_descriptor){
                                                                    anyhow::bail!("Can't use file input slot to appoint file out path.");
                                                                }
                                                            }
                                                        }
                                                    }
                                                    if input_slot_descriptor.is_none(){
                                                      anyhow::bail!("No such text input slot with descriptor: {text_input_descriptor}")
                                                    }
                                                }
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                            PreviewCollectFrom::FileOut {
                                output_material_descriptor: filesome_out_material
                                    .unwrap()
                                    .descriptor,
                            }
                        }
                        CollectFrom::Stdout => PreviewCollectFrom::Stdout,
                        CollectFrom::Stderr => PreviewCollectFrom::Stderr,
                    };
                    // 解析收集到哪里去
                    let to = match collected_out.to.to_owned() {
                        CollectTo::Text => PreviewCollectTo::Text,
                        _ => anyhow::bail!(
                            "Mismatched collected_out type with descriptor: {}",
                            collected_out.descriptor
                        ),
                    };
                    // 解析收集规则
                    let rule = match collected_out.collecting.to_owned() {
                        CollectRule::Regex(regex) => PreviewCollectRule::Regex { pattern: regex },
                        CollectRule::BottomLines(line_count) => {
                            PreviewCollectRule::BottomLines { count: line_count }
                        }
                        CollectRule::TopLines(line_count) => {
                            PreviewCollectRule::TopLines { count: line_count }
                        }
                    };

                    collect_previews.push(CollectPreview {
                        from,
                        rule,
                        to,
                        optional: *optional,
                    });
                }
                OutputSlot::File {
                    origin, optional, ..
                } => {
                    match origin {
                        FileOutOrigin::CollectedOut(collector_descriptor) => {
                            let collected_out = collected_outs
                                .iter()
                                .find(|el| el.descriptor.eq(collector_descriptor))
                                .ok_or(anyhow::anyhow!(
                                    "No such collect out with descriptor: {collector_descriptor}"
                                ))?;
                            // 解析从哪收集
                            let from = match collected_out.from.to_owned() {
                                CollectFrom::FileOut(fileout_descriptor) => {
                                    let mut filesome_out_material = Option::<FilesomeOutput>::None;
                                    for out_slot in usecase_spec.output_slots.iter() {
                                        if let OutputSlot::File {
                                            descriptor, origin, ..
                                        } = out_slot
                                        {
                                            if descriptor.eq(&fileout_descriptor) {
                                                match origin {
                                                    FileOutOrigin::CollectedOut(_) =>
                                                        anyhow::bail!(
                                                            "Collect CollectedOut file or text is not implemented!"
                                                        ),
                                                    FileOutOrigin::UsecaseOut(
                                                        file_out_and_appointed_by,
                                                    ) => {
                                                        filesome_out_material = Some(filesome_output_materials.iter().find(|el|el.descriptor.eq(&file_out_and_appointed_by.file_out_material_descriptor)).ok_or(anyhow::anyhow!("No such file out material with descriptor: {}",file_out_and_appointed_by.file_out_material_descriptor))?.to_owned());
                                                        break;
                                                    }
                                                }
                                            }
                                        }
                                    }

                                    PreviewCollectFrom::FileOut {
                                        output_material_descriptor: filesome_out_material
                                            .unwrap()
                                            .descriptor,
                                    }
                                }
                                CollectFrom::Stdout => PreviewCollectFrom::Stdout,
                                CollectFrom::Stderr => PreviewCollectFrom::Stderr,
                            };
                            // 解析收集到哪里去
                            let to = match collected_out.to.to_owned() {
                                CollectTo::File(out_file) => PreviewCollectTo::File {
                                    path: out_file.get_path(),
                                },
                                _ => anyhow::bail!(
                                    "Mismatched collected_out type with descriptor: {}",
                                    collected_out.descriptor
                                ),
                            };
                            // 解析收集规则
                            let rule = match collected_out.collecting.to_owned() {
                                CollectRule::Regex(regex) => {
                                    PreviewCollectRule::Regex { pattern: regex }
                                }
                                CollectRule::BottomLines(line_count) => {
                                    PreviewCollectRule::BottomLines { count: line_count }
                                }
                                CollectRule::TopLines(line_count) => {
                                    PreviewCollectRule::TopLines { count: line_count }
                                }
                            };
                            collect_previews.push(CollectPreview {
                                from,
                                rule,
                                to,
                                optional: *optional,
                            });
                        }
                        FileOutOrigin::UsecaseOut(file_out_and_appointed_by) => {
                            let filesome_output = filesome_output_materials
                                .iter()
                                .find(|el| {
                                    el.descriptor
                                        .eq(&file_out_and_appointed_by.file_out_material_descriptor)
                                })
                                .ok_or(anyhow::anyhow!(
                                    "No such filesome output with descriptor: {}",
                                    file_out_and_appointed_by.file_out_material_descriptor
                                ))?
                                .to_owned();
                            let out_path_alter_descriptor =
                                match file_out_and_appointed_by.kind.to_owned() {
                                    AppointedBy::Material => None,
                                    AppointedBy::InputSlot {
                                        text_input_descriptor,
                                    } => Some(text_input_descriptor.to_owned()),
                                };
                            match out_path_alter_descriptor {
                                Some(text_descriptor) => {
                                    file_infos.push(FileInfoPreview::DynamicOutput {
                                        from: InDescriptor::InputSlot {
                                            descriptor: text_descriptor.to_owned(),
                                        },
                                        output_material_descriptor: filesome_output
                                            .descriptor
                                            .to_owned(),
                                    });
                                }
                                None => {
                                    file_infos.push(FileInfoPreview::ConstOutput {
                                        output_material_descriptor: filesome_output.descriptor,
                                        path: match filesome_output.file_kind.to_owned() {
                                            FileKind::Normal(path) => path,
                                            FileKind::Batched(path) => path,
                                        },
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }

        let mut argument_formats_sorts2 = BTreeMap::new();
        if !argument_formats_sorts.is_empty() {
            let mut argument_formats_sorts = argument_formats_sorts.into_iter().collect::<Vec<_>>();
            argument_formats_sorts.sort_by(|a, b| a.0.cmp(&b.0));
            let mut p: usize = 0;
            if argument_formats_sorts.get(0).unwrap().0.ne(&p) {
                anyhow::bail!("sort must begin with '0'");
            }

            for argument_format_sort in argument_formats_sorts.iter().skip(1) {
                let current = argument_format_sort.0;
                if current - p != 1 {
                    anyhow::bail!("sort {current} is bigger more than 1 compared with {p}.");
                }
                p = argument_format_sort.0;
            }
            argument_formats_sorts2 = argument_formats_sorts
                .into_iter()
                .collect::<BTreeMap<usize, FormatFillPreview>>()
        }

        Ok(CommandPreview {
            software_facility: FacilityKind::from(software_spec),
            command_name: usecase_spec.command_file,
            argument_formats_sorts: argument_formats_sorts2,
            environment_formats,
            templates_kv_map,
            std_in,
            file_infos,
            collect_previews,
        })
    }
}

impl Default for PackageValidateService {
    fn default() -> Self {
        Self::new()
    }
}

impl PackageValidateService {
    pub fn new() -> Self {
        Self {}
    }

    /// 根据参数描述符获得参数值 format、以及初始化表示该 format 各占位符填充值的 HashMap
    ///
    /// # 参数
    ///
    /// * `argument_materials` - 软件包中参数材料列表
    /// * `descriptor` - 参数描述符
    fn argument_format(
        argument_materials: &[Argument],
        descriptor: &str,
    ) -> anyhow::Result<FormatFillPreview> {
        let value_format = argument_materials
            .iter()
            .find(|el| el.descriptor.eq(descriptor))
            .ok_or(anyhow::anyhow!(
                "No such argument material descriptor: {descriptor}"
            ))?
            .value_format
            .to_owned();
        Ok(FormatFillPreview {
            format: value_format,
            placeholder_fill_map: HashMap::new(),
        })
    }

    /// 根据环境变量描述符获得键、参数值 format、以及初始化表示该 format 各占位符填充值的 HashMap
    ///
    /// # 参数
    ///
    /// * `environment_materials` - 软件包中环境变量材料列表
    /// * `descriptor` - 环境变量描述符
    fn environment_kv_format(
        environment_materials: &[Environment],
        descriptor: &str,
    ) -> anyhow::Result<(String, FormatFillPreview)> {
        let environment =
            environment_materials.iter().find(|el| el.descriptor.eq(descriptor)).ok_or(
                anyhow::anyhow!("No such environment material with descriptor: {descriptor}"),
            )?;
        Ok((
            environment.key.to_owned(),
            FormatFillPreview {
                format: environment.value_format.to_owned(),
                placeholder_fill_map: HashMap::new(),
            },
        ))
    }
}
#[cfg(test)]
mod tests {
    use std::{collections::HashMap, fs::File};

    use super::{IPackageValidateService, ValidateData};
    use crate::{
        dtos::prelude::*,
        models::{
            package::{Package, SoftwareData, UsecaseData},
            software_computing_usecase::SoftwareComputingUsecase,
        },
        services::parse_node::{IParseNodeService, ParseNodeService},
    };

    const TEST_DIR: &str = "test_contents";
    const USECASE: &str = "vasp_usecase";
    const SOFTWARE: &str = "vasp_std";
    #[tokio::test]
    async fn tt() {
        // create_tars();
        let us = std::fs::read(format!("{TEST_DIR}/{USECASE}.tar")).unwrap();
        let sf = std::fs::read(format!("{TEST_DIR}/{SOFTWARE}.tar")).unwrap();
        let p1 = Package::extract_package(uuid::Uuid::new_v4(), &us).unwrap();
        let p2 = Package::extract_package(uuid::Uuid::new_v4(), &sf).unwrap();
        let ps = Packages::SoftwareComputing(p1, p2);
        let scu = SoftwareComputingUsecase::extract_packages(ps);
        let software_data = SoftwareData {
            spec: scu.software_spec,
            arguments: scu.arguments,
            environments: scu.environments,
            filesome_inputs: scu.filesome_inputs,
            filesome_outputs: scu.filesome_outputs,
        };
        let usecase_data = UsecaseData {
            spec: scu.usecase_spec,
            collected_outs: scu.collected_outs,
            template_file_infos: scu.template_file_infos,
        };
        let vd = ValidateData {
            software_data,
            usecase_data,
        };
        // let vdd = serde_json::to_string_pretty(&vd).unwrap();
        // println!("{vdd}");
        let service = super::PackageValidateService {};
        match service.package_validate(vd).await {
            Ok(cp) => {
                println!("{cp:#?}")
            }
            Err(e) => {
                eprintln!("{e}")
            }
        };

        // let package_path = PathBuf::from("abcabc");
        // match PackageValidateService::read_yml(package_path.join(MANIFEST_FILE_NAME)).await {
        //     Ok(s) => println!("{s}"),
        //     Err(e) => eprintln!("{e}"),
        // }
    }
    #[tokio::test]
    async fn test_get_node_draft() {
        update_tars();
        let us = std::fs::read(format!("{TEST_DIR}/{USECASE}.tar")).unwrap();
        let sf = std::fs::read(format!("{TEST_DIR}/{SOFTWARE}.tar")).unwrap();
        let p1 = Package::extract_package(uuid::Uuid::new_v4(), &us).unwrap();
        let p2 = Package::extract_package(uuid::Uuid::new_v4(), &sf).unwrap();
        let node_ability =
            NodeAbilityKind::extract_packages(Packages::SoftwareComputing(p1, p2)).unwrap();
        let x = ParseNodeService::parse_node(node_ability).await;
        println!("{x:#?}");
    }

    #[test]
    fn update_tars() {
        std::fs::remove_file(format!("{TEST_DIR}/{USECASE}.tar")).unwrap();
        std::fs::remove_file(format!("{TEST_DIR}/{SOFTWARE}.tar")).unwrap();
        let usecase_buf = File::create(format!("{TEST_DIR}/{USECASE}.tar")).unwrap();
        let software_buf = File::create(format!("{TEST_DIR}/{SOFTWARE}.tar")).unwrap();

        let mut usecase_tar_builder = tar::Builder::new(usecase_buf);
        let mut software_tar_builder = tar::Builder::new(software_buf);

        usecase_tar_builder
            .append_dir_all(USECASE, format!("{TEST_DIR}/{USECASE}"))
            .unwrap();
        usecase_tar_builder.finish().unwrap();
        software_tar_builder
            .append_dir_all(SOFTWARE, format!("{TEST_DIR}/{SOFTWARE}"))
            .unwrap();
        software_tar_builder.finish().unwrap();
    }
    #[test]
    fn asd() {
        let mut x = HashMap::new();
        x.insert("asd", Some("asf"));
        println!("{x:#?}");
    }
}
