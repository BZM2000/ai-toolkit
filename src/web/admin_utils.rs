/// Returns a sanitized redirect target for module admin pages to prevent arbitrary redirects.
pub fn sanitize_module_redirect(input: Option<&str>) -> &'static str {
    match input {
        Some("/dashboard/modules/summarizer") => "/dashboard/modules/summarizer",
        Some("/dashboard/modules/translatedocx") => "/dashboard/modules/translatedocx",
        Some("/dashboard/modules/grader") => "/dashboard/modules/grader",
        _ => "/dashboard",
    }
}

/// Compose a flash message HTML snippet for known admin status or error codes.
pub fn compose_flash_message(status: Option<&str>, error: Option<&str>) -> String {
    if let Some(status) = status {
        let message = match status {
            "created" => "已成功创建用户。",
            "password_updated" => "已更新密码。",
            "glossary_created" => "已新增术语。",
            "glossary_updated" => "已更新术语。",
            "glossary_deleted" => "已删除术语。",
            "topic_saved" => "已保存主题。",
            "topic_deleted" => "已删除主题。",
            "journal_saved" => "已保存期刊参考。",
            "journal_deleted" => "已删除期刊参考。",
            "summarizer_models_saved" => "已更新摘要模块模型。",
            "summarizer_prompts_saved" => "已更新摘要模块提示词。",
            "docx_models_saved" => "已更新 DOCX 模块模型。",
            "docx_prompts_saved" => "已更新 DOCX 模块提示词。",
            "grader_models_saved" => "已更新稿件评估模型。",
            "grader_prompts_saved" => "已更新稿件评估提示词。",
            "group_created" => "已创建额度组。",
            "group_saved" => "已更新额度组。",
            "group_assigned" => "已更新用户额度组。",
            _ => "",
        };

        if !message.is_empty() {
            return format!(r#"<div class="flash success">{message}</div>"#);
        }
    }

    if let Some(error) = error {
        let message = match error {
            "duplicate" => "用户名已存在。",
            "not_authorized" => "需要管理员权限。",
            "missing_username" => "请输入用户名。",
            "missing_password" => "请输入密码。",
            "password_missing" => "请输入新密码。",
            "user_missing" => "未找到该用户。",
            "hash_failed" => "处理密码时出错，请重试。",
            "glossary_missing_fields" => "请填写英文和中文术语。",
            "glossary_duplicate" => "已存在相同英文术语。",
            "glossary_not_found" => "未找到对应术语。",
            "topic_missing_name" => "请填写主题名称。",
            "topic_not_found" => "未找到对应主题。",
            "journal_missing_name" => "请填写期刊名称。",
            "journal_invalid_low" => "请输入有效的低区间数值。",
            "journal_invalid_score" => "主题分值必须是 0-2 的整数。",
            "journal_not_found" => "未找到对应期刊参考。",
            "summarizer_invalid_models" => "请提供摘要模块所需的全部模型字段。",
            "summarizer_invalid_prompts" => "请填写摘要模块的所有提示文案。",
            "docx_invalid_models" => "请提供 DOCX 模块的模型配置。",
            "docx_invalid_prompts" => "请填写 DOCX 模块的提示文案。",
            "grader_invalid_models" => "请提供稿件评估模块的模型配置。",
            "grader_invalid_prompts" => "请填写稿件评估模块的提示文案。",
            "group_missing" => "请选择有效的额度组。",
            "group_invalid" => "额度组标识无效。",
            "group_invalid_limit" => "额度上限需为非负整数。",
            "group_duplicate" => "已存在同名额度组。",
            "group_name_missing" => "请输入额度组名称。",
            _ => "发生未知错误，请查看日志。",
        };

        return format!(r#"<div class="flash error">{message}</div>"#);
    }

    String::new()
}
