use super::*;
use structure::Piece;

impl Parser<'_> {
    pub(super) fn block_scalar(
        &mut self,
        header: &str,
        parent: usize,
        depth: usize,
        start: usize,
    ) -> Result<DataNodeId, Diagnostic> {
        let style = header.as_bytes()[0];
        let indicators = header[1..].trim();
        let loc = self.build.loc(start..start + header.len());
        self.build.reserve(depth, loc)?;
        let mut chomping = None;
        let mut explicit = None;
        for c in indicators.chars() {
            match c {
                '+' | '-' if chomping.is_none() => chomping = Some(c),
                '1'..='9' if explicit.is_none() => {
                    explicit = Some(parent + c.to_digit(10).unwrap() as usize)
                }
                _ => return Err(Diagnostic::error("invalid YAML block scalar header", loc)),
            }
        }
        let inferred = (self.position..self.lines.len())
            .find_map(|i| {
                if self.content(i).trim().is_empty() {
                    None
                } else {
                    Some(self.lines[i].indent)
                }
            })
            .filter(|indent| *indent > parent);
        let indent = explicit.or(inferred).unwrap_or(parent + 1);
        let pieces_start = self.build.plan.pieces.len();
        let mut length = 0;
        let mut pending_newlines = 0usize;
        let mut previous: Option<(bool, bool)> = None;
        let mut end = start + header.len();
        while let Some(line) = self.lines.get(self.position).copied() {
            let content = self.content(self.position);
            if !content.trim().is_empty() && line.indent < indent {
                break;
            }
            let range = if line.indent <= parent && content.trim().is_empty() {
                line.end..line.end
            } else {
                (line.start + indent).min(line.end)..line.end
            };
            let empty = range.is_empty();
            let more = line.indent > indent;
            let piece_loc = self.build.loc(line.start..line.end);
            if let Some((prev_empty, prev_more)) = previous {
                if style == b'|' || prev_empty || empty || prev_more || more {
                    pending_newlines += 1;
                } else {
                    self.flush_newlines(&mut length, &mut pending_newlines, piece_loc)?;
                    self.piece(&mut length, Piece::Spaces(1), piece_loc)?;
                }
            }
            if !empty {
                self.flush_newlines(&mut length, &mut pending_newlines, piece_loc)?;
                self.piece(&mut length, Piece::Source(range), piece_loc)?;
            }
            previous = Some((empty, more));
            end = line.end;
            self.position += 1;
        }
        match chomping {
            Some('-') => {}
            Some('+') => {
                pending_newlines += 1;
                self.flush_newlines(&mut length, &mut pending_newlines, loc)?;
            }
            _ if previous.is_some() => self.piece(&mut length, Piece::Newlines(1), loc)?,
            _ => {}
        }
        Ok(self.build.plan.scalar(
            Scalar::String(Text::Block(pieces_start..self.build.plan.pieces.len())),
            self.build.loc(start..end),
        ))
    }
    fn piece(&mut self, length: &mut usize, piece: Piece, loc: Location) -> Result<(), Diagnostic> {
        let bytes = match &piece {
            Piece::Source(range) => range.len(),
            Piece::Spaces(n) | Piece::Newlines(n) => *n,
        };
        self.build.admit(length, bytes, false, loc)?;
        self.build.plan.pieces.push(piece);
        Ok(())
    }
    fn flush_newlines(
        &mut self,
        length: &mut usize,
        count: &mut usize,
        loc: Location,
    ) -> Result<(), Diagnostic> {
        if *count > 0 {
            self.piece(length, Piece::Newlines(*count), loc)?;
        }
        *count = 0;
        Ok(())
    }
}
