use crate::{
	layout::WidgetID,
	parser::{
		ParserContext, ParserFile, iter_attribs, parse_children, parse_widget_universal,
		style::parse_style,
	},
	widget::div::WidgetDiv,
};

pub fn parse_widget_div<'a, U1, U2>(
	file: &ParserFile,
	ctx: &mut ParserContext<U1, U2>,
	node: roxmltree::Node<'a, 'a>,
	parent_id: WidgetID,
) -> anyhow::Result<()> {
	let attribs: Vec<_> = iter_attribs(file, ctx, &node, false).collect();
	let style = parse_style(&attribs);

	let (new_id, _) = ctx
		.layout
		.add_child(parent_id, WidgetDiv::create(), style)?;

	parse_widget_universal(file, ctx, node, new_id);
	parse_children(file, ctx, node, new_id)?;

	Ok(())
}
