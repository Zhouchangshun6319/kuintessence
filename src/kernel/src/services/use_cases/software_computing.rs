use crate::prelude::*;
use handlebars::Handlebars;
use lib_co_repo::{
    client::IInfoGetter,
    models::prelude::{
        AppointedBy, Argument, CollectFrom as RepoCollectFrom, CollectRule as RepoCollectRule,
        CollectTo as RepoCollectTo, Environment, FileKind, FileOutOrigin, FileRef, InputSlot,
        OutputSlot, SoftwareSpec as RepoSoftwareSpec, TextRef,
    },
};
use serde::Serialize;
use std::{collections::HashMap, sync::Arc};

/// 输入内容
#[derive(Clone)]
struct InContent {
    /// 字符串
    pub literal: String,
    /// 输入文件
    pub infiles: Vec<FileInfo>,
}

/// 格式填充
#[derive(Debug)]
struct FormatFill {
    /// 格式
    pub format: String,
    /// 每个占位符用什么填充
    pub placeholder_fill_map: HashMap<usize, Option<String>>,
}

/// 软件用例解析微服务
#[derive(Builder)]
pub struct SoftwareComputingUsecaseService {
    /// 软件用例获取器
    computing_usecase_getter: Arc<dyn IInfoGetter + Send + Sync>,
    /// 文本仓储
    text_storage_repository: Arc<dyn ITextStorageRepository + Send + Sync>,
    /// 任务分发服务
    task_distribution_service: Arc<dyn ITaskDistributionService + Send + Sync>,
    /// 软件黑名单仓储
    software_block_list_repository: Arc<dyn ISoftwareBlockListRepository + Send + Sync>,
    /// 已安装软件仓储
    installed_software_repository: Arc<dyn IInstalledSoftwareRepository + Send + Sync>,
    /// 集群仓储
    cluster_repository: Arc<dyn IClusterRepository + Send + Sync>,
    /// 节点实例仓储
    node_instance_repository: Arc<dyn INodeInstanceRepository + Send + Sync>,
    workflow_instance_repository: Arc<dyn IWorkflowInstanceRepository + Send + Sync>,
}

#[async_trait]
impl IUsecaseService for SoftwareComputingUsecaseService {
    async fn handle_usecase(&self, node_spec: NodeSpec) -> anyhow::Result<()> {
        let task = self.parse_task(node_spec).await?;
        let cluster_id = self.cluster_repository.get_random_cluster().await?;
        let mut node_instance =
            self.node_instance_repository.get_by_id(&task.id.to_string()).await?;
        node_instance.cluster_id = Some(cluster_id.to_owned());
        self.node_instance_repository.update(node_instance).await?;
        self.node_instance_repository.save_changed().await?;
        self.task_distribution_service.send_task(&task, cluster_id).await
    }

    async fn operate_task(&self, operate: Operation) -> anyhow::Result<()> {
        let cluster_id = self
            .node_instance_repository
            .get_by_id(&operate.task_id.to_string())
            .await?
            .cluster_id
            .ok_or(anyhow::anyhow!("Node instance without cluster id!"))?;
        let command = operate.command;
        let task = Task {
            id: operate.task_id,
            body: vec![],
            command,
        };
        self.task_distribution_service.send_task(&task, cluster_id).await
    }
    fn get_service_type(&self) -> NodeInstanceKind {
        NodeInstanceKind::SoftwareUsecaseComputing
    }
    async fn get_cmd(&self, node_id: Uuid) -> anyhow::Result<Option<String>> {
        let flow_id = self
            .node_instance_repository
            .get_by_id(&node_id.to_string())
            .await?
            .flow_instance_id;
        let flow = self.workflow_instance_repository.get_by_id(&flow_id.to_string()).await?;
        let node_spec = flow.spec.node(node_id).to_owned();
        let task = self.parse_task(node_spec).await?;
        let name_and_arguments = task.body.iter().find_map(|el| {
            if let TaskBody::UsecaseExecution {
                name, arguments, ..
            } = el
            {
                Some((name, arguments))
            } else {
                None
            }
        });
        Ok(match name_and_arguments {
            Some((name, arguments)) => {
                let arg_str = arguments.join(" ");
                Some(if arg_str.is_empty() {
                    name.to_string()
                } else {
                    format!("{name} {arg_str}")
                })
            }
            None => None,
        })
    }
}

impl SoftwareComputingUsecaseService {
    /// 解析节点数据，返回任务
    ///
    /// # 参数
    ///
    /// * `node_spec` - 节点数据
    async fn parse_task(&self, node_spec: NodeSpec) -> anyhow::Result<Task> {
        let data = match &node_spec.kind {
            NodeKind::SoftwareUsecaseComputing { data } => data,
            _ => anyhow::bail!("Unreachable node kind!"),
        };

        let (usecase_version_id, software_version_id) = (
            data.usecase_version_id.to_owned(),
            data.software_version_id.to_owned(),
        );

        // 根据用例包 id、软件包 id，获取用例分析数据
        let computing_usecase = self
            .computing_usecase_getter
            .get_computing_usecase(software_version_id, usecase_version_id)
            .await?;

        let usecase_spec = computing_usecase.usecase_spec;
        let argument_materials = computing_usecase.arguments;
        let environment_materials = computing_usecase.environments;
        let filesome_input_materials = computing_usecase.filesome_inputs;
        let filesome_output_materials = computing_usecase.filesome_outputs;
        let software_spec = computing_usecase.software_spec;
        let template_file_infos = computing_usecase.template_file_infos;
        let collected_outs = computing_usecase.collected_outs;
        let requirements = usecase_spec.requirements;
        let override_requirements = &node_spec.requirements;

        let mut argument_formats_sorts = HashMap::<usize, FormatFill>::new();
        let mut environment_formats_map = HashMap::<String, FormatFill>::new();
        let mut files = vec![];
        let mut std_in = StdInKind::default();

        // 模板描述符及其键填充值的对应关系集合
        let mut templates_kv_json = HashMap::<String, HashMap<String, Option<String>>>::new();

        let mut task = Task {
            id: node_spec.id.to_owned(),
            command: TaskCommand::Start,
            body: vec![],
        };

        for (argument_material_descriptor, sort) in usecase_spec.flag_arguments.iter() {
            let value = Self::argument_format(&argument_materials, argument_material_descriptor);
            argument_formats_sorts.entry(*sort).or_insert(value);
        }

        for environment_material_descriptor in usecase_spec.flag_environments.iter() {
            let (key, value) = Self::environment_kv_format(
                &environment_materials,
                environment_material_descriptor,
            );
            environment_formats_map.entry(key).or_insert(value);
        }

        for input_slot in usecase_spec.input_slots.iter() {
            // 找到该输入插槽的输入
            let in_content = self.get_content(&node_spec, input_slot.descriptor()).await?;

            if let Some(in_content) = in_content.to_owned() {
                files.extend(in_content.infiles.iter().map(|el| el.to_owned()));
            }

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
                                    Self::argument_format(&argument_materials, descriptor);

                                argument_formats_sorts
                                    .entry(*sort)
                                    .or_insert(argument_format)
                                    .placeholder_fill_map
                                    .insert(
                                        *placeholder_nth,
                                        in_content.to_owned().map(|el| el.literal),
                                    );
                            }

                            TextRef::EnvRef {
                                descriptor,
                                placeholder_nth,
                            } => {
                                let (key, value_format) =
                                    Self::environment_kv_format(&environment_materials, descriptor);

                                environment_formats_map
                                    .entry(key)
                                    .or_insert(value_format)
                                    .placeholder_fill_map
                                    .insert(
                                        *placeholder_nth,
                                        in_content.to_owned().map(|el| el.literal),
                                    );
                            }

                            TextRef::StdIn => {
                                std_in = StdInKind::Text {
                                    text: in_content.to_owned().unwrap().literal,
                                };
                            }

                            TextRef::TemplateRef {
                                descriptor,
                                ref_keys,
                            } => {
                                for ref_key in ref_keys.iter() {
                                    templates_kv_json
                                        .entry(descriptor.to_owned())
                                        .or_insert(HashMap::new())
                                        .insert(
                                            ref_key.to_owned(),
                                            in_content.to_owned().map(|el| el.literal),
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
                                    Self::argument_format(&argument_materials, descriptor);

                                argument_formats_sorts
                                    .entry(*sort)
                                    .or_insert(argument_format)
                                    .placeholder_fill_map
                                    .insert(
                                        *placeholder_nth,
                                        in_content.to_owned().map(|el| el.literal),
                                    );
                            }
                            FileRef::EnvRef {
                                descriptor,
                                placeholder_nth,
                            } => {
                                let (key, value_format) =
                                    Self::environment_kv_format(&environment_materials, descriptor);

                                environment_formats_map
                                    .entry(key)
                                    .or_insert(value_format)
                                    .placeholder_fill_map
                                    .insert(
                                        *placeholder_nth,
                                        in_content.to_owned().map(|el| el.literal),
                                    );
                            }

                            FileRef::StdIn => {
                                std_in = match in_content.to_owned() {
                                    Some(x) => StdInKind::File { path: x.literal },
                                    None => StdInKind::None,
                                };
                            }

                            FileRef::FileInputRef(_) => {
                                // 在 `get_content` 中处理过了，这里不需要再处理
                            }

                            FileRef::TemplateRef {
                                descriptor,
                                ref_keys,
                            } => {
                                for ref_key in ref_keys.iter() {
                                    templates_kv_json
                                        .entry(descriptor.to_owned())
                                        .or_insert(HashMap::new())
                                        .insert(
                                            ref_key.to_owned(),
                                            in_content.to_owned().map(|el| el.literal),
                                        );
                                }
                            }
                        }
                    }
                }
            }
        }

        // 遍历使用的 template
        for (template_descriptor, template_kv_json) in templates_kv_json.iter() {
            let using_template_file = usecase_spec
                .template_files
                .iter()
                .find(|el| el.descriptor.eq(template_descriptor))
                .unwrap();
            let template_file_info = template_file_infos
                .iter()
                .find(|el| el.descriptor.eq(template_descriptor))
                .unwrap();

            let file_name = template_file_info.file_name.to_owned();
            let filled_result =
                Self::get_template_file_result(&template_file_info.content, template_kv_json)?;

            for as_content in using_template_file.as_content.iter() {
                match as_content {
                    TextRef::ArgRef {
                        descriptor,
                        placeholder_nth,
                        sort,
                    } => {
                        let argument_format =
                            Self::argument_format(&argument_materials, descriptor);

                        argument_formats_sorts
                            .entry(*sort)
                            .or_insert(argument_format)
                            .placeholder_fill_map
                            .insert(*placeholder_nth, Some(filled_result.to_owned()));
                    }

                    TextRef::EnvRef {
                        descriptor,
                        placeholder_nth,
                    } => {
                        let (key, value_format) =
                            Self::environment_kv_format(&environment_materials, descriptor);

                        environment_formats_map
                            .entry(key)
                            .or_insert(value_format)
                            .placeholder_fill_map
                            .insert(*placeholder_nth, Some(filled_result.to_owned()));
                    }

                    TextRef::StdIn => {
                        std_in = StdInKind::Text {
                            text: filled_result.to_owned(),
                        };
                    }

                    TextRef::TemplateRef {
                        descriptor: _,
                        ref_keys: _,
                    } => todo!(),
                }
            }

            if !(using_template_file.as_file_name.is_empty()
                || using_template_file.as_file_name.len() == 1
                    && matches!(
                        using_template_file.as_file_name.get(0).unwrap(),
                        FileRef::FileInputRef(_)
                    ))
            {
                files.push(FileInfo::Input {
                    path: file_name.to_owned(),
                    is_package: false,
                    form: InFileForm::Content(filled_result.to_owned()),
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
                            Self::argument_format(&argument_materials, descriptor);

                        argument_formats_sorts
                            .entry(*sort)
                            .or_insert(argument_format)
                            .placeholder_fill_map
                            .insert(*placeholder_nth, Some(file_name.to_owned()));
                    }

                    FileRef::EnvRef {
                        descriptor,
                        placeholder_nth,
                    } => {
                        let (key, value_format) =
                            Self::environment_kv_format(&environment_materials, descriptor);

                        environment_formats_map
                            .entry(key)
                            .or_insert(value_format)
                            .placeholder_fill_map
                            .insert(*placeholder_nth, Some(file_name.to_owned()));
                    }

                    FileRef::StdIn => {
                        std_in = StdInKind::File {
                            path: file_name.to_owned(),
                        }
                    }

                    FileRef::FileInputRef(input_material_descriptor) => {
                        match filesome_input_materials
                            .iter()
                            .find(|el| el.descriptor.eq(input_material_descriptor))
                            .unwrap()
                            .file_kind
                            .to_owned()
                        {
                            FileKind::Normal(wild_card) => files.push(FileInfo::Input {
                                path: wild_card,
                                is_package: false,
                                form: InFileForm::Content(filled_result.to_owned()),
                            }),
                            FileKind::Batched(wild_card) => files.push(FileInfo::Input {
                                path: wild_card,
                                is_package: true,
                                form: InFileForm::Content(filled_result.to_owned()),
                            }),
                        }
                    }

                    FileRef::TemplateRef {
                        descriptor: _,
                        ref_keys: _,
                    } => todo!(),
                }
            }
        }

        for output_slot in usecase_spec.output_slots.iter() {
            match output_slot {
                OutputSlot::Text {
                    collected_out_descriptor,
                    optional,
                    descriptor,
                    ..
                } => {
                    let collected_out = collected_outs
                        .iter()
                        .find(|el| el.descriptor.eq(collected_out_descriptor))
                        .unwrap();
                    // 解析从哪收集
                    let from = match collected_out.from.to_owned() {
                        RepoCollectFrom::FileOut(fileout_descriptor) => {
                            // 因为文件名有可能被通过输入插槽改写，不能直接使用软件包中定义的输出文件默认名字
                            let mut path = String::default();
                            for out_slot in usecase_spec.output_slots.iter() {
                                if let OutputSlot::File {
                                    descriptor, origin, ..
                                } = out_slot
                                {
                                    if descriptor.eq(&fileout_descriptor) {
                                        match origin {
                                            FileOutOrigin::CollectedOut(_) => todo!(),
                                            FileOutOrigin::UsecaseOut(
                                                file_out_and_appointed_by,
                                            ) => {
                                                let filesome_output = filesome_output_materials
                                                    .iter()
                                                    .find(|el| {
                                                        el.descriptor.eq(&file_out_and_appointed_by
                                                            .file_out_material_descriptor)
                                                    })
                                                    .unwrap();
                                                let out_path_alter =
                                                    match file_out_and_appointed_by.kind.to_owned()
                                                    {
                                                        AppointedBy::Material => None,
                                                        AppointedBy::InputSlot {
                                                            text_input_descriptor,
                                                        } => {
                                                            match node_spec
                                                                .input_slot(&text_input_descriptor)
                                                                .kind
                                                                .to_owned()
                                                            {
                                                                NodeInputSlotKind::Text {
                                                                    contents,
                                                                    ..
                                                                } => Some(
                                                                    self.text_storage_repository
                                                                        .get_by_id(
                                                                            &contents
                                                                                .unwrap()
                                                                                .get(0)
                                                                                .unwrap()
                                                                                .to_string(),
                                                                        )
                                                                        .await?
                                                                        .value,
                                                                ),
                                                                _ => unreachable!(),
                                                            }
                                                        }
                                                    };
                                                path =
                                                    out_path_alter.unwrap_or(match filesome_output
                                                        .file_kind
                                                        .to_owned()
                                                    {
                                                        FileKind::Normal(path) => path,
                                                        FileKind::Batched(path) => path,
                                                    });
                                                break;
                                            }
                                        }
                                    }
                                }
                            }

                            CollectFrom::FileOut { path }
                        }
                        RepoCollectFrom::Stdout => CollectFrom::Stdout,
                        RepoCollectFrom::Stderr => CollectFrom::Stderr,
                    };
                    // 解析收集到哪里去
                    let to = match collected_out.to.to_owned() {
                        RepoCollectTo::Text => {
                            let id = node_spec
                                .output_slots
                                .iter()
                                .find(|el| el.descriptor.eq(descriptor))
                                .unwrap()
                                .all_tasks_text_outputs()?
                                .get(0)
                                .unwrap()
                                .to_owned();
                            // self.text_storage_repository
                            //     .insert(TextStorage {
                            //         key: id.to_owned(),
                            //         value: "".to_string(),
                            //     })
                            //     .await?;
                            CollectTo::Text { id }
                        }
                        _ => unreachable!(),
                    };
                    // 解析收集规则
                    let rule = match collected_out.collecting.to_owned() {
                        RepoCollectRule::Regex(regex) => CollectRule::Regex(regex),
                        RepoCollectRule::BottomLines(line_count) => {
                            CollectRule::BottomLines(line_count)
                        }
                        RepoCollectRule::TopLines(line_count) => CollectRule::TopLines(line_count),
                    };
                    task.body.push(TaskBody::CollectedOut {
                        from,
                        rule,
                        to,
                        optional: *optional,
                    });
                }
                OutputSlot::File {
                    descriptor: usecase_outslot_descriptor,
                    origin,
                    optional,
                    ..
                } => {
                    let task_output_slot = node_spec.output_slot(usecase_outslot_descriptor);
                    match origin {
                        FileOutOrigin::CollectedOut(collector_descriptor) => {
                            let collected_out = collected_outs
                                .iter()
                                .find(|el| el.descriptor.eq(collector_descriptor))
                                .unwrap();
                            // 解析从哪收集
                            let from = match collected_out.from.to_owned() {
                                RepoCollectFrom::FileOut(fileout_descriptor) => {
                                    // 因为文件名有可能被通过输入插槽改写，不能直接使用软件包中定义的输出文件默认名字
                                    let mut path = String::default();
                                    for out_slot in usecase_spec.output_slots.iter() {
                                        if let OutputSlot::File {
                                            descriptor, origin, ..
                                        } = out_slot
                                        {
                                            if descriptor.eq(&fileout_descriptor) {
                                                match origin {
                                                    FileOutOrigin::CollectedOut(_) => {
                                                        todo!()
                                                    }
                                                    FileOutOrigin::UsecaseOut(
                                                        file_out_and_appointed_by,
                                                    ) => {
                                                        let filesome_output = filesome_output_materials.iter().find(|el|el.descriptor.eq(&file_out_and_appointed_by.file_out_material_descriptor)).unwrap();
                                                        let out_path_alter =
                                                            match file_out_and_appointed_by
                                                                .kind
                                                                .to_owned()
                                                            {
                                                                AppointedBy::Material => None,
                                                                AppointedBy::InputSlot {
                                                                    text_input_descriptor,
                                                                } => {
                                                                    match node_spec.input_slot(&text_input_descriptor).kind.to_owned(){
                                                                            NodeInputSlotKind::Text { contents, .. } => {
                                                                                Some(self.text_storage_repository.get_by_id(&contents.unwrap().get(0).unwrap().to_string()).await?.value)
                                                                            },
                                                                            _ => unreachable!()
                                                                        }
                                                                }
                                                            };
                                                        path = out_path_alter.unwrap_or(
                                                            match filesome_output
                                                                .file_kind
                                                                .to_owned()
                                                            {
                                                                FileKind::Normal(path) => path,
                                                                FileKind::Batched(path) => path,
                                                            },
                                                        );
                                                        break;
                                                    }
                                                }
                                            }
                                        }
                                    }

                                    CollectFrom::FileOut { path }
                                }
                                RepoCollectFrom::Stdout => CollectFrom::Stdout,
                                RepoCollectFrom::Stderr => CollectFrom::Stderr,
                            };
                            // 解析收集到哪里去
                            let to = match collected_out.to.to_owned() {
                                RepoCollectTo::File(out_file) => {
                                    let id = node_spec
                                        .output_slots
                                        .iter()
                                        .find(|el| el.descriptor.eq(usecase_outslot_descriptor))
                                        .unwrap()
                                        .all_tasks_file_outputs()?
                                        .get(0)
                                        .unwrap()
                                        .to_owned();

                                    CollectTo::File {
                                        path: out_file.get_path(),
                                        id,
                                    }
                                }
                                _ => unreachable!(),
                            };
                            // 解析收集规则
                            let rule = match collected_out.collecting.to_owned() {
                                RepoCollectRule::Regex(regex) => CollectRule::Regex(regex),
                                RepoCollectRule::BottomLines(line_count) => {
                                    CollectRule::BottomLines(line_count)
                                }
                                RepoCollectRule::TopLines(line_count) => {
                                    CollectRule::TopLines(line_count)
                                }
                            };
                            task.body.push(TaskBody::CollectedOut {
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
                                .unwrap();
                            let out_path_alter = match file_out_and_appointed_by.kind.to_owned() {
                                AppointedBy::Material => None,
                                AppointedBy::InputSlot {
                                    text_input_descriptor,
                                } => {
                                    match node_spec
                                        .input_slot(&text_input_descriptor)
                                        .kind
                                        .to_owned()
                                    {
                                        NodeInputSlotKind::Text { contents, .. } => Some(
                                            self.text_storage_repository
                                                .get_by_id(
                                                    &contents.unwrap().get(0).unwrap().to_string(),
                                                )
                                                .await?
                                                .value,
                                        ),
                                        _ => unreachable!(),
                                    }
                                }
                            };
                            match &filesome_output.file_kind {
                                FileKind::Normal(file_name) => {
                                    let out_file_id =
                                        task_output_slot.all_tasks_file_outputs()?.get(0).unwrap();
                                    files.push(FileInfo::Output {
                                        id: out_file_id.to_owned(),
                                        path: out_path_alter.unwrap_or(file_name.to_owned()),
                                        is_package: false,
                                        optional: *optional,
                                    });
                                }
                                FileKind::Batched(wild_card) => {
                                    let out_file_or_zip_id =
                                        task_output_slot.all_tasks_file_outputs()?.get(0).unwrap();
                                    files.push(FileInfo::Output {
                                        id: out_file_or_zip_id.to_owned(),
                                        path: out_path_alter.unwrap_or(wild_card.to_owned()),
                                        is_package: true,
                                        optional: *optional,
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
        let mut argument_formats_sorts = argument_formats_sorts.into_iter().collect::<Vec<_>>();
        argument_formats_sorts.sort_by(|a, b| a.0.cmp(&b.0));
        let arguments = argument_formats_sorts
            .iter_mut()
            .map(|(_, format_fill)| {
                let format = &mut format_fill.format;
                let placeholder_fill_map = &format_fill.placeholder_fill_map;
                let mut placeholder_fill_vec = placeholder_fill_map.iter().collect::<Vec<_>>();
                placeholder_fill_vec.sort_by(|a, b| a.0.cmp(b.0));
                for placeholder_fill in placeholder_fill_vec.iter() {
                    *format = format.replacen(
                        "{{}}",
                        if let Some(x) = placeholder_fill.1 {
                            x
                        } else {
                            ""
                        },
                        1,
                    );
                }
                format.to_owned()
            })
            .collect();

        let environments = environment_formats_map
            .iter_mut()
            .map(|(key, format_fill)| {
                let format = &mut format_fill.format;
                let placeholder_fill_map = &format_fill.placeholder_fill_map;
                let mut placeholder_fill_vec = placeholder_fill_map.iter().collect::<Vec<_>>();
                placeholder_fill_vec.sort_by(|a, b| a.0.cmp(b.0));
                for placeholder_fill in placeholder_fill_vec.iter() {
                    *format = format.replacen(
                        "{{}}",
                        if let Some(x) = placeholder_fill.1 {
                            x
                        } else {
                            ""
                        },
                        1,
                    );
                }
                (key.to_owned(), format.to_owned())
            })
            .collect();

        task.body.insert(
            0,
            TaskBody::UsecaseExecution {
                name: usecase_spec.command_file.to_owned(),
                arguments,
                environments,
                files,
                facility_kind: FacilityKind::from(software_spec.to_owned()),
                std_in,
                requirements: override_requirements.to_owned().or(serde_json::from_str::<
                    Option<Requirements>,
                >(
                    &serde_json::to_string(&requirements)?,
                )?),
            },
        );

        let (software_name, version, require_install_arguments) = match software_spec.to_owned() {
            RepoSoftwareSpec::Spack {
                name,
                argument_list,
            } => (
                name,
                argument_list.get(0).cloned().unwrap_or_default().replace('@', ""),
                argument_list,
            ),
            RepoSoftwareSpec::Singularity { .. } => {
                (String::default(), String::default(), Vec::default())
            }
        };

        if !self
            .software_block_list_repository
            .is_software_version_blocked(&software_name, &version)
            .await?
            && self
                .installed_software_repository
                .is_software_satisfied(&software_name, &require_install_arguments)
                .await?
        {
            task.body.insert(
                0,
                TaskBody::SoftwareDeployment {
                    facility_kind: FacilityKind::from(software_spec.to_owned()),
                },
            );
        }

        println!("{task:#?}");
        Ok(task)
    }

    /// 返回模板填充完毕后的内容
    ///
    /// # 参数
    ///
    /// * `template_content` - 模板内容
    /// * `kv_json` - 模板内容填充键值对
    fn get_template_file_result<T>(template_content: &str, kv_json: T) -> anyhow::Result<String>
    where
        T: Serialize,
    {
        let mut reg = Handlebars::new();
        reg.register_template_string("template_content", template_content)?;
        Ok(reg.render("template_content", &kv_json)?)
    }

    /// 根据参数描述符获得参数值 format、以及初始化表示该 format 各占位符填充值的 HashMap
    ///
    /// # 参数
    ///
    /// * `argument_materials` - 软件包中参数材料列表
    /// * `descriptor` - 参数描述符
    fn argument_format(argument_materials: &[Argument], descriptor: &str) -> FormatFill {
        let value_format = argument_materials
            .iter()
            .find(|el| el.descriptor.eq(descriptor))
            .unwrap()
            .value_format
            .to_owned();
        FormatFill {
            format: value_format,
            placeholder_fill_map: HashMap::new(),
        }
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
    ) -> (String, FormatFill) {
        let environment =
            environment_materials.iter().find(|el| el.descriptor.eq(descriptor)).unwrap();
        (
            environment.key.to_owned(),
            FormatFill {
                format: environment.value_format.to_owned(),
                placeholder_fill_map: HashMap::new(),
            },
        )
    }

    /// 得到节点实例某输入插槽上的输入
    async fn get_content(
        &self,
        node_spec: &NodeSpec,
        input_slot_descriptor: &str,
    ) -> anyhow::Result<Option<InContent>> {
        let node_input_slot = node_spec.input_slot(input_slot_descriptor);
        // 如果输入插槽没有输入且该输入插槽的输入是可选的
        if node_input_slot.is_empty_input() && node_input_slot.optional {
            return Ok(None);
        }

        match node_input_slot.kind.clone() {
            NodeInputSlotKind::Text { contents, .. } => {
                let mut texts = vec![];

                for content in contents.as_ref().unwrap() {
                    texts.push(
                        self.text_storage_repository.get_by_id(&content.to_string()).await?.value,
                    )
                }
                Ok(Some(InContent {
                    literal: texts.join(" "),
                    infiles: vec![],
                }))
            }
            NodeInputSlotKind::File {
                contents,
                expected_file_name,
                is_batch,
            } => {
                let mut file_names = vec![];
                let mut file_infos = vec![];
                for content in contents.as_ref().unwrap().iter() {
                    if let Some(ref expected_file_name) = expected_file_name {
                        file_names.push(expected_file_name.to_owned());
                        file_infos.push(FileInfo::Input {
                            form: InFileForm::Id(content.file_metadata_id.to_owned()),
                            path: expected_file_name.to_owned(),
                            is_package: is_batch,
                        });
                    } else {
                        file_names.push(content.file_metadata_name.to_owned());
                        file_infos.push(FileInfo::Input {
                            form: InFileForm::Id(content.file_metadata_id.to_owned()),
                            path: content.file_metadata_name.to_owned(),
                            is_package: is_batch,
                        });
                    }
                }
                Ok(Some(InContent {
                    literal: file_names.join(" "),
                    infiles: file_infos,
                }))
            }
            NodeInputSlotKind::Unknown => unreachable!(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::prelude::*;
    use alice_architecture::repository::IReadOnlyRepository;
    use lib_co_repo::dtos::prelude::Packages;
    use lib_co_repo::models::package::Package;
    use lib_co_repo::models::software_computing_usecase::SoftwareComputingUsecase;
    use std::str::FromStr;
    use uuid::Uuid;

    async fn load() -> (Arc<SoftwareComputingUsecaseService>, Arc<JSONRepository>) {
        let json_repository =
            JSONRepository::new("C:\\Users\\Zooey\\JsonRepository").await.unwrap();
        let json_repository = Arc::new(json_repository);
        let mut computing_usecase_getter = MockComputingUsecaseGetter::new();
        computing_usecase_getter.expect_get_computing_usecase().returning(|_, _| {
            let software_tar =
                std::fs::read(r#"C:\Users\zooey\Codes\Work\SchemaTest\Repos\calculating.tar"#)
                    .unwrap();
            let usecase_tar =
                std::fs::read(r#"C:\Users\zooey\Codes\Work\SchemaTest\Repos\us1.tar"#).unwrap();
            let software = Package::extract_package(
                Uuid::from_str("8b243827-e5ea-4653-808d-47aa4ce30e99").unwrap(),
                &software_tar,
            )
            .unwrap();
            let usecase = Package::extract_package(
                Uuid::from_str("ccc8a4ca-0396-4f31-b683-79261140a429").unwrap(),
                &usecase_tar,
            )
            .unwrap();
            let packages = Packages::SoftwareComputing(software, usecase);
            Ok(SoftwareComputingUsecase::extract_packages(packages))
        });

        let computing_usecase_getter = Arc::new(computing_usecase_getter);

        let mut text_storage_repository = MockTextStorageRepository::new();
        text_storage_repository.expect_get_by_id().returning(|_| {
            Ok(TextStorage {
                key: Some(Uuid::parse_str("0ec4e324-0d51-434e-baf5-36424a8e75f5").unwrap()),
                value: "5".to_string(),
            })
        });
        text_storage_repository.expect_insert().returning(Ok);
        text_storage_repository.expect_save_changed().returning(|| Ok(true));
        let text_storage_repository = Arc::new(text_storage_repository);

        let mut software_block_list_repository = MockSoftwareBlockListRepository::new();
        software_block_list_repository
            .expect_is_software_version_blocked()
            .returning(|_, _| Ok(false));
        let software_block_list_repository = Arc::new(software_block_list_repository);

        let mut installed_software_repository = MockInstalledSoftwareRepository::new();
        installed_software_repository
            .expect_is_software_satisfied()
            .returning(|_, _| Ok(true));
        let installed_software_repository = Arc::new(installed_software_repository);

        let mut cluster_repository = MockClusterRepository::new();
        cluster_repository
            .expect_get_random_cluster()
            .returning(|| Ok(uuid::Uuid::new_v4()));
        let cluster_repository = Arc::new(cluster_repository);

        let mut node_instance_repository = MockNodeInstanceRepository::new();
        node_instance_repository
            .expect_get_by_id()
            .returning(|_| Ok(NodeInstance::default()));
        node_instance_repository.expect_update().returning(Ok);
        node_instance_repository.expect_save_changed().returning(|| Ok(true));
        let node_instance_repository = Arc::new(node_instance_repository);

        let mut task_distribution_service = MockTaskDistributionService::new();
        task_distribution_service.expect_send_task().returning(|_, _| Ok(()));
        let task_distribution_service = Arc::new(task_distribution_service);

        (
            Arc::new(
                SoftwareComputingUsecaseServiceBuilder::default()
                    .computing_usecase_getter(computing_usecase_getter)
                    .text_storage_repository(text_storage_repository)
                    .task_distribution_service(task_distribution_service)
                    .software_block_list_repository(software_block_list_repository)
                    .installed_software_repository(installed_software_repository)
                    .cluster_repository(cluster_repository)
                    .node_instance_repository(node_instance_repository)
                    .build()
                    .unwrap(),
            ),
            json_repository,
        )
    }

    #[tokio::test]
    pub async fn test_compute_usecase() {
        let (computing_usecase_service, json_repository) = load().await;
        let workflow_instances = (json_repository
            as Arc<dyn IReadOnlyRepository<WorkflowInstance> + Send + Sync>)
            .get_all()
            .await
            .unwrap();
        let workflow_instance = workflow_instances.get(0).unwrap();
        let node_spec = workflow_instance.spec.node_specs.get(1).unwrap();
        computing_usecase_service.handle_usecase(node_spec.clone()).await.unwrap();
    }
}
