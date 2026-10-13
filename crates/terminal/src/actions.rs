// Based on iced_term 0.8 (https://github.com/Harzu/iced_term), MIT License,
// Copyright (c) Ilya Shvyryalkin. Modified for codemux. See NOTICE.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Action {
    Shutdown,
    ChangeTitle(String),
    ChildExit(i32),
    #[default]
    Ignore,
}
