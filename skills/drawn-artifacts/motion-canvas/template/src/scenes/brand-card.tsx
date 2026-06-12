import {Layout, Rect, Txt, makeScene2D} from '@motion-canvas/2d';
import {all, createRef, sequence, waitFor} from '@motion-canvas/core';

const gold = '#C8A84E';
const navy = '#070D17';
const ivory = '#F2EDE3';

export default makeScene2D(function* (view) {
  const panel = createRef<Rect>();
  const eyebrow = createRef<Txt>();
  const title = createRef<Txt>();
  const rule = createRef<Rect>();

  view.fill('rgba(0, 0, 0, 0)');
  view.add(
    <Layout width={1920} height={1080} alignItems="center" justifyContent="center">
      <Rect
        ref={panel}
        width={1380}
        height={620}
        radius={18}
        fill={navy}
        opacity={0}
        scale={0.94}
        padding={72}
        direction="column"
        justifyContent="center"
        gap={34}
      >
        <Txt
          ref={eyebrow}
          text="MONTAGE DRAWN ARTIFACT"
          fill={gold}
          fontFamily="Inter, Arial, sans-serif"
          fontSize={38}
          fontWeight={800}
          letterSpacing={0}
          opacity={0}
          y={-24}
        />
        <Txt
          ref={title}
          text="Replace this with the episode beat"
          fill={ivory}
          fontFamily="Inter, Arial, sans-serif"
          fontSize={82}
          fontWeight={900}
          textAlign="center"
          lineHeight={92}
          width={1100}
          opacity={0}
          y={30}
        />
        <Rect ref={rule} width={0} height={8} radius={4} fill={gold} opacity={0.92} />
      </Rect>
    </Layout>,
  );

  yield* all(panel().opacity(0.88, 0.35), panel().scale(1, 0.35));
  yield* sequence(
    0.12,
    all(eyebrow().opacity(1, 0.3), eyebrow().y(0, 0.3)),
    all(title().opacity(1, 0.38), title().y(0, 0.38)),
    rule().width(640, 0.35),
  );
  yield* waitFor(1.2);
  yield* all(panel().opacity(0, 0.3), title().opacity(0, 0.22), eyebrow().opacity(0, 0.22));
});
