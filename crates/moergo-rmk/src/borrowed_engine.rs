use rmk::lighting::service::{
    CommandResult, Invalidation, LightingEngine, RenderInput, RenderOutcome,
};

/// Keep the engine's tables out of the task's constructor and poll frames.
pub struct BorrowedEngine<E: 'static>(pub &'static mut E);

impl<S, E: LightingEngine<S>> LightingEngine<S> for BorrowedEngine<E> {
    type Frame = E::Frame;
    type Input = E::Input;
    type Command = E::Command;
    type Reply = E::Reply;
    type Error = E::Error;

    fn on_input(&mut self, input: Self::Input, snapshot: &S) -> Result<Invalidation, Self::Error> {
        self.0.on_input(input, snapshot)
    }

    #[inline(never)]
    fn handle_command(
        &mut self,
        now_ms: u64,
        command: Self::Command,
        snapshot: &S,
    ) -> Result<CommandResult<Self::Reply>, Self::Error> {
        self.0.handle_command(now_ms, command, snapshot)
    }

    fn render(
        &mut self,
        input: RenderInput<'_, S>,
        frame: &mut Self::Frame,
    ) -> Result<RenderOutcome, Self::Error> {
        self.0.render(input, frame)
    }

    fn on_presented(&mut self, frame: &Self::Frame) {
        self.0.on_presented(frame);
    }
}
