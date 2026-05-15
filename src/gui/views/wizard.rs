//! 创建向导视图（Mock）

/// 创建向导视图结构
pub struct WizardView {
    pub current_step: u8,
    pub total_steps: u8,
}

impl WizardView {
    pub fn new() -> Self {
        Self {
            current_step: 1,
            total_steps: 7,
        }
    }
    
    pub fn next_step(&mut self) {
        if self.current_step < self.total_steps {
            self.current_step += 1;
        }
    }
    
    pub fn prev_step(&mut self) {
        if self.current_step > 1 {
            self.current_step -= 1;
        }
    }
}

impl Default for WizardView {
    fn default() -> Self {
        Self::new()
    }
}
